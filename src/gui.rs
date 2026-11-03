//! Natiivi käyttöliittymä (egui, englanniksi): Colorize, Repair, Exemplar, Stabilization & output, Add-ons, Assistant.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};
use serde::{Deserialize, Serialize};

use crate::kuva::{Bgr, Kentta};
use crate::resepti::{KIRJAIMET, Kiinnipiste, Projekti, Referenssit, Resepti, SIIRTO, ehdokkaat};
use crate::tuotanto::{Asetukset, aja_resepti, ehdota_kiinnipisteet, kohtausrajat};
use crate::vari::{bgr_lab, kroma, lab_bgr};
use crate::video::{Tiedot, aika_sekunneiksi, aikateksti, hae_kehys, sovelluskansio};
use crate::{Viesti, mallit};

#[cfg(feature = "itsetesti")]
mod itsetesti;

const ESIKATSELU_LEVEYS: usize = 640;
const PIKKU: f32 = 160.0;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Tallennettavat {
    v_video: String,
    v_ulos: String,
    v_resepti: Resepti,
    v_aika: String,
    v_aanilahde: String,
    k_video: String,
    k_alkuperainen: String,
    k_ulos: String,
    k_taytto: bool,
    k_tayta_malli: String,
    k_harmaa_raja: f32,
    k_aanilahde: String,
    e_projekti: String,
    e_ulos: String,
    e_ddcolor: String,
    e_automaattisia: usize,
    e_aanilahde: String,
    laite: String,
    tyoleveys: usize,
    ikkuna: usize,
    virtaus: bool,
    tasaus_voima: f32,
    muisti: f32,
    kohtausmuisti: bool,
    lukitus: f32,
    lukitus_iho: bool,
    lukitus_taivas: bool,
    lukitus_kasvit: bool,
    leikkaus_raja: f32,
    koodekki: String,
    crf: u32,
    // näyte ei tallennu: unohtunut näyte saisi seuraavan ajon käsittelemään vain pätkän
    #[serde(skip)]
    nayte_alku: f64,
    #[serde(skip)]
    nayte_kesto: f64,
    /// Väritä-välilehti: väritys -> tarkistus -> korjaus (putki)
    v_putki: bool,
    /// Väritä-välilehti: kopioruudut pudotetaan / väri edellisestä kuvasta
    v_kopiot: bool,
    nako_malli: String,
    nako_suosittele: bool,
    nako_valitse: bool,
    nako_naytteita: usize,
    laatu_video: String,
    esik_kesto: f64,
    esik_vertaa: bool,
    analyysi_min_kesto: f64,
    analyysi_enintaan: usize,
}

impl Default for Tallennettavat {
    fn default() -> Self {
        Tallennettavat {
            v_video: String::new(),
            v_ulos: String::new(),
            v_resepti: Resepti::default(),
            v_aika: "0:00:10".into(),
            v_aanilahde: String::new(),
            k_video: String::new(),
            k_alkuperainen: String::new(),
            k_ulos: String::new(),
            k_taytto: true,
            k_tayta_malli: "ddcolor_suomi_technicolor".into(),
            k_harmaa_raja: 4.0,
            k_aanilahde: String::new(),
            e_projekti: String::new(),
            e_ulos: String::new(),
            e_ddcolor: "ddcolor_suomi".into(),
            e_automaattisia: 10,
            e_aanilahde: String::new(),
            laite: "auto".into(),
            tyoleveys: 960,
            ikkuna: 7,
            virtaus: true,
            tasaus_voima: 1.0,
            muisti: 0.98,
            kohtausmuisti: true,
            lukitus: 0.6,
            lukitus_iho: true,
            lukitus_taivas: true,
            lukitus_kasvit: true,
            leikkaus_raja: 0.35,
            koodekki: "x264".into(),
            crf: 14,
            nayte_alku: 0.0,
            nayte_kesto: 0.0,
            v_putki: true,
            v_kopiot: true,
            nako_malli: crate::nako::OLETUSMALLI.into(),
            nako_suosittele: true,
            nako_valitse: false,
            nako_naytteita: 40,
            laatu_video: String::new(),
            esik_kesto: 5.0,
            esik_vertaa: true,
            analyysi_min_kesto: 1.0,
            analyysi_enintaan: 0,
        }
    }
}

fn asetustiedosto() -> PathBuf {
    sovelluskansio().join("bw2color_settings.json")
}

enum GuiViesti {
    Kehys(String, usize, Option<Bgr>),
    Ehdotukset(usize, Vec<(usize, Option<Resepti>, Option<Bgr>, String)>, bool),
    VEsikatselu(Bgr, String),
    Automaattiset(Vec<usize>, usize),
    AjoValmis(std::result::Result<String, String>),
    TyoValmis(Option<String>),
    LisaosaValmis,
    OllamaTila(crate::lisaosat::OllamaTila),
    Suositus(usize, std::result::Result<crate::nako::Suositus, String>),
    Nakomallit(std::result::Result<Vec<String>, String>),
    Laatu(std::result::Result<(PathBuf, Vec<crate::nako::Havainto>), String>),
    /// kohtausanalyysin valmis kiinnipiste (projektipolku, kiinnipiste)
    AnalyysiKp(String, Kiinnipiste),
    AnalyysiValmis(std::result::Result<String, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Valilehti {
    Varita,
    Korjaa,
    Exemplar,
    Asetukset,
    Lisaosat,
    Avustaja,
}

/// Zoomattava kuvanäkymä.
#[derive(Default)]
struct Nakyma {
    zoom: f32,
    keski: Vec2,
}

impl Nakyma {
    fn piirra(&mut self, ui: &mut egui::Ui, tex: Option<&TextureHandle>, teksti: &str, koko: Vec2) {
        if self.zoom < 1.0 {
            self.zoom = 1.0;
            self.keski = Vec2::splat(0.5);
        }
        let (vastaus, maalari) = ui.allocate_painter(koko, Sense::click_and_drag());
        let alue = vastaus.rect;
        maalari.rect_filled(alue, 0.0, Color32::from_gray(28));
        let Some(tex) = tex else {
            maalari.text(alue.center(), egui::Align2::CENTER_CENTER, if teksti.is_empty() { "No image" } else { teksti },
                         egui::FontId::proportional(14.0), Color32::GRAY);
            return;
        };
        let [iw, ih] = tex.size();
        let s = (alue.width() / iw as f32).min(alue.height() / ih as f32);
        let kohde = Rect::from_center_size(alue.center(), Vec2::new(iw as f32 * s, ih as f32 * s));
        if vastaus.hovered() {
            let rulla = ui.input(|i| i.smooth_scroll_delta.y);
            if rulla != 0.0 {
                let vanha = self.zoom;
                self.zoom = (self.zoom * if rulla > 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(1.0, 16.0);
                if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                    // pidä hiiren alla oleva kohta paikallaan
                    let suht = (p - kohde.center()) / kohde.size();
                    self.keski += suht * (1.0 / vanha - 1.0 / self.zoom);
                }
            }
        }
        if vastaus.dragged() && self.zoom > 1.0 {
            self.keski -= vastaus.drag_delta() / kohde.size() / self.zoom;
        }
        if vastaus.double_clicked() {
            self.zoom = 1.0;
            self.keski = Vec2::splat(0.5);
        }
        let puoli = 0.5 / self.zoom;
        self.keski = Vec2::new(self.keski.x.clamp(puoli, 1.0 - puoli), self.keski.y.clamp(puoli, 1.0 - puoli));
        let uv = Rect::from_min_max(Pos2::new(self.keski.x - puoli, self.keski.y - puoli), Pos2::new(self.keski.x + puoli, self.keski.y + puoli));
        maalari.image(tex.id(), kohde, uv, Color32::WHITE);
        if !teksti.is_empty() {
            maalari.text(alue.min + Vec2::new(8.0, 6.0), egui::Align2::LEFT_TOP, teksti, egui::FontId::proportional(15.0), Color32::WHITE);
        }
        if self.zoom > 1.0 {
            maalari.text(Pos2::new(alue.max.x - 8.0, alue.min.y + 6.0), egui::Align2::RIGHT_TOP, format!("{:.1}x", self.zoom),
                         egui::FontId::proportional(14.0), Color32::from_rgb(255, 208, 96));
        }
    }
}

type Tyo = Box<dyn FnOnce(Sender<GuiViesti>) -> Result<()> + Send + 'static>;

/// Satunnaisluku välillä [a, b] (kellosta siemennetty, riittää esikatselukohdan arpomiseen).
fn satunnainen(a: f64, b: f64) -> f64 {
    let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    a + (b - a).max(0.0) * ((x >> 11) as f64 / (1u64 << 53) as f64)
}

/// Esikatselusoitin: toistaa väritetyn pätkän silmukassa, valinnaisesti rinnakkain lähdevideon kanssa.
struct Soitin {
    ulos: String,
    lahde: String,
    alku: f64,
    kesto: f64,
    vertaa: bool,
    rx: Receiver<Bgr>,
    seis: Arc<AtomicBool>,
    tauko: Arc<AtomicBool>,
    tex: Option<TextureHandle>,
}

impl Soitin {
    fn uusi(ctx: &egui::Context, ulos: &str, lahde: &str, alku: f64, kesto: f64, vertaa: bool) -> Soitin {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Bgr>(2);
        let seis = Arc::new(AtomicBool::new(false));
        let tauko = Arc::new(AtomicBool::new(false));
        let (s2, p2, c2) = (seis.clone(), tauko.clone(), ctx.clone());
        let (u, l) = (ulos.to_string(), lahde.to_string());
        std::thread::spawn(move || {
            let toista = || -> Result<()> {
                let tu = Tiedot::lue(&u)?;
                let mut tl = Tiedot::lue(&l)?;
                if (tl.fps - tu.fps).abs() > 0.01 {
                    // tulos on kirjoitettu todellisella nopeudella (kopioruudut pudotettu)
                    if let Ok(h) = Tiedot::lue_harvennettu(&l) {
                        if (h.fps - tu.fps).abs() <= 0.01 {
                            tl = h;
                        }
                    }
                }
                let koko = tu.koko_enintaan(if vertaa { 480 } else { 720 });
                let vali = std::time::Duration::from_secs_f64(1.0 / tu.fps.max(1.0));
                while !s2.load(Ordering::Relaxed) {
                    let mut lu = crate::video::Lukija::avaa(&tu, 0, 0, Some(koko))?;
                    let mut ll = if vertaa { Some(crate::video::Lukija::avaa(&tl, tl.kehys(alku), tu.n, Some(koko))?) } else { None };
                    let mut seuraava = std::time::Instant::now();
                    while let Some(k) = lu.seuraava() {
                        while p2.load(Ordering::Relaxed) && !s2.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            seuraava = std::time::Instant::now();
                        }
                        if s2.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        let kuva = match ll.as_mut().and_then(|l| l.seuraava()) {
                            Some(m) => Bgr::vierekkain(&[&m, &k]),
                            None => k,
                        };
                        let nyt = std::time::Instant::now();
                        if seuraava > nyt {
                            std::thread::sleep(seuraava - nyt);
                        }
                        seuraava += vali;
                        if tx.send(kuva).is_err() {
                            return Ok(());
                        }
                        c2.request_repaint();
                    }
                }
                Ok(())
            };
            if let Err(e) = toista() {
                crate::loki(&format!("Preview playback failed: {e}"));
            }
        });
        Soitin { ulos: ulos.into(), lahde: lahde.into(), alku, kesto, vertaa, rx, seis, tauko, tex: None }
    }
}

impl Drop for Soitin {
    fn drop(&mut self) {
        self.seis.store(true, Ordering::Relaxed);
    }
}

struct Ehdotus {
    resepti: Resepti,
    kuva: Bgr,
    tex: TextureHandle,
}

pub struct Sovellus {
    s: Tallennettavat,
    valilehti: Valilehti,
    tila_valilehti: Valilehti,
    tx: Sender<GuiViesti>,
    rx: Receiver<GuiViesti>,
    loki: Arc<Mutex<Vec<String>>>,
    tila: Arc<Mutex<(String, f32)>>,
    ajo: Option<Arc<AtomicBool>>,
    tyo_kaynnissa: bool,
    tyojono: std::collections::VecDeque<(String, Tyo)>,
    refit: Referenssit,
    // väritä
    v_tex: Option<TextureHandle>,
    v_teksti: String,
    v_nakyma: Nakyma,
    // exemplar
    projekti: Projekti,
    e_tiedot: Option<Tiedot>,
    kohta: usize,
    kehys: Option<Bgr>,
    kehys_tex: Option<TextureHandle>,
    hakupyynto: Arc<Mutex<Option<(Tiedot, usize)>>>,
    haku_kaynnissa: Arc<AtomicBool>,
    ehdotukset: HashMap<String, Ehdotus>,
    virheet: HashMap<String, String>,
    ehdotus_kehys: Option<usize>,
    ehdotukset_jonossa: bool,
    ab_muisti: Arc<Mutex<HashMap<(usize, String), Kentta>>>,
    f_pohja: String,
    hsv: (f32, f32, f32),
    hsv_muuttui: bool,
    kp_ref_voima: f32,
    nayta_vari: bool,
    hover: Option<String>,
    e_nakyma: Nakyma,
    e_varoitus: String,
    tarkistettu_video: String,
    e_aika: String,
    valitun_tex: Option<(usize, TextureHandle)>,
    // lisäosat: käynnissä oleva asennus (kuvaus, pysäytys)
    lisaosa_ajo: Option<(String, Arc<AtomicBool>)>,
    ollama: Option<crate::lisaosat::OllamaTila>,
    ollama_haku: bool,
    nako_veto: String,
    // näkömalliavustaja
    suositus: Option<(usize, std::result::Result<crate::nako::Suositus, String>)>,
    suositus_kaynnissa: Option<usize>,
    nakomallit: Vec<String>,
    nakomallit_haettu: bool,
    laatu_ajo: Option<Arc<AtomicBool>>,
    laatu_tulos: Option<(PathBuf, Vec<crate::nako::Havainto>)>,
    viimeisin_tulos: String,
    // satunnainen esikatselu: (tulos, lähde, alku, kesto)
    esikatselu_odottaa: Option<(String, String, f64, f64)>,
    soitin: Option<Soitin>,
    ctx: egui::Context,
    analyysi_ajo: Option<Arc<AtomicBool>>,
    #[cfg(feature = "itsetesti")]
    testi: Option<itsetesti::Testi>,
}

pub fn kaynnista() -> Result<()> {
    let ikoni = image::load_from_memory(include_bytes!("../varita.png")).ok().map(|i| {
        let i = i.to_rgba8();
        egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
    });
    let mut nakyma = egui::ViewportBuilder::default()
        .with_title("BW2Color Movie – Colorize · Repair · Exemplar")
        .with_inner_size([1300.0, 820.0])
        .with_min_inner_size([1000.0, 660.0]);
    if let Some(i) = ikoni {
        nakyma = nakyma.with_icon(Arc::new(i));
    }
    let asetukset = eframe::NativeOptions { viewport: nakyma, ..Default::default() };
    eframe::run_native("varita", asetukset, Box::new(|cc| Ok(Box::new(Sovellus::new(cc)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn pick(otsikko: &str, suodatin: &[(&str, &[&str])], alku: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new().set_title(otsikko);
    for (n, p) in suodatin {
        d = d.add_filter(*n, p);
    }
    if let Some(k) = Path::new(alku).parent().filter(|p| p.is_dir()) {
        d = d.set_directory(k);
    }
    d.pick_file().map(|p| p.to_string_lossy().into_owned())
}

fn tallenna_nimella(otsikko: &str, suodatin: &[(&str, &[&str])], oletus: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new().set_title(otsikko);
    for (n, p) in suodatin {
        d = d.add_filter(*n, p);
    }
    let p = Path::new(oletus);
    if let Some(k) = p.parent().filter(|p| p.is_dir()) {
        d = d.set_directory(k);
    }
    if let Some(n) = p.file_name() {
        d = d.set_file_name(n.to_string_lossy());
    }
    d.save_file().map(|p| p.to_string_lossy().into_owned())
}

const VIDEOT: &[&str] = &["mp4", "mkv", "avi", "mov", "m4v", "webm", "mpg", "ts"];
const KUVAT: &[&str] = &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"];

/// Itsetestissä dialogit ohitetaan: virheet kirjataan ja kysymyksiin vastataan kyllä.
pub static AUTOMAATTI: AtomicBool = AtomicBool::new(false);

fn virhe(teksti: &str) {
    if AUTOMAATTI.load(Ordering::Relaxed) {
        crate::loki(&format!("[dialogi] {teksti}"));
        return;
    }
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("BW2Color Movie").set_description(teksti).show();
}

fn kysy(otsikko: &str, teksti: &str) -> bool {
    if AUTOMAATTI.load(Ordering::Relaxed) {
        crate::loki(&format!("[kysymys] {otsikko}"));
        return true;
    }
    rfd::MessageDialog::new()
        .set_title(otsikko)
        .set_description(teksti)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

fn tekstuuri(ctx: &egui::Context, nimi: &str, k: &Bgr) -> TextureHandle {
    ctx.load_texture(nimi, egui::ColorImage::from_rgb([k.w, k.h], &k.rgb()), egui::TextureOptions::LINEAR)
}

fn ehdota_ulos(video: &str, paate: &str) -> String {
    let p = Path::new(video);
    p.with_file_name(format!("{}{paate}.mkv", p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default())).to_string_lossy().into()
}

fn tiedostorivi(ui: &mut egui::Ui, nimi: &str, arvo: &mut String, valitse: impl FnOnce() -> Option<String>) -> bool {
    let mut muuttui = false;
    ui.label(nimi);
    ui.horizontal(|ui| {
        let lev = ui.available_width() - 80.0;
        ui.add_sized([lev.max(100.0), 22.0], egui::TextEdit::singleline(arvo));
        if ui.button("Browse…").clicked() {
            if let Some(p) = valitse() {
                *arvo = p;
                muuttui = true;
            }
        }
    });
    ui.end_row();
    muuttui
}

fn menetelman_nimi(m: &str) -> String {
    mallit::kuvaus(m).unwrap_or_else(|| "Colour transfer from a reference (no model)".into())
}

impl Sovellus {
    fn new(cc: &eframe::CreationContext) -> Self {
        let s: Tallennettavat = std::fs::read_to_string(asetustiedosto()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let (tx, rx) = channel();
        let loki = Arc::new(Mutex::new(Vec::<String>::new()));
        let tila = Arc::new(Mutex::new(("Ready.".to_string(), 0.0f32)));
        {
            let (loki, tila, ctx) = (loki.clone(), tila.clone(), cc.egui_ctx.clone());
            crate::aseta_vastaanottaja(Box::new(move |v| {
                match v {
                    Viesti::Loki(t) => {
                        let mut l = loki.lock().unwrap();
                        l.push(t);
                        let n = l.len();
                        if n > 3000 {
                            l.drain(0..n - 3000);
                        }
                    }
                    Viesti::Tila(t, i, n) => *tila.lock().unwrap() = (t, i as f32 / n.max(1) as f32),
                }
                ctx.request_repaint();
            }));
        }
        let mut sov = Sovellus {
            s,
            valilehti: Valilehti::Varita,
            tila_valilehti: Valilehti::Varita,
            tx,
            rx,
            loki,
            tila,
            ajo: None,
            tyo_kaynnissa: false,
            tyojono: Default::default(),
            refit: Referenssit::default(),
            v_tex: None,
            v_teksti: String::new(),
            v_nakyma: Nakyma::default(),
            projekti: Projekti::default(),
            e_tiedot: None,
            kohta: 0,
            kehys: None,
            kehys_tex: None,
            hakupyynto: Arc::new(Mutex::new(None)),
            haku_kaynnissa: Arc::new(AtomicBool::new(false)),
            ehdotukset: HashMap::new(),
            virheet: HashMap::new(),
            ehdotus_kehys: None,
            ehdotukset_jonossa: false,
            ab_muisti: Arc::new(Mutex::new(HashMap::new())),
            f_pohja: "A".into(),
            hsv: (0.0, 1.0, 1.0),
            hsv_muuttui: false,
            kp_ref_voima: 0.7,
            nayta_vari: false,
            hover: None,
            e_nakyma: Nakyma::default(),
            e_varoitus: String::new(),
            tarkistettu_video: String::new(),
            e_aika: "0:00:00".into(),
            valitun_tex: None,
            lisaosa_ajo: None,
            ollama: None,
            ollama_haku: false,
            nako_veto: crate::nako::OLETUSMALLI.into(),
            suositus: None,
            suositus_kaynnissa: None,
            nakomallit: Vec::new(),
            nakomallit_haettu: false,
            laatu_ajo: None,
            laatu_tulos: None,
            viimeisin_tulos: String::new(),
            esikatselu_odottaa: None,
            soitin: None,
            ctx: cc.egui_ctx.clone(),
            analyysi_ajo: None,
            #[cfg(feature = "itsetesti")]
            testi: std::env::var("VARITA_TESTI").ok().map(|k| {
                AUTOMAATTI.store(true, Ordering::Relaxed);
                itsetesti::Testi::new(PathBuf::from(k))
            }),
        };
        if !(0.05..=0.95).contains(&sov.s.leikkaus_raja) {
            crate::loki(&format!("Scene cut sensitivity {} was out of range – reset to the default 0.35.", sov.s.leikkaus_raja));
            sov.s.leikkaus_raja = 0.35;
        }
        let puuttuvat: Vec<&str> = mallit::MALLIT.iter().filter(|m| !mallit::on_ladattu(m.nimi)).map(|m| m.nimi).collect();
        crate::loki(&format!("Application folder {}. Models installed {}/{}.", sovelluskansio().display(),
                             mallit::MALLIT.len() - puuttuvat.len(), mallit::MALLIT.len()));
        if crate::lisaosat::puuttuu_pakollisia() {
            crate::loki("First start: install FFmpeg and at least one DDColor model on the Add-ons tab.");
            sov.valilehti = Valilehti::Lisaosat;
        }
        if !sov.s.e_projekti.is_empty() && Path::new(&sov.s.e_projekti).is_file() {
            let p = sov.s.e_projekti.clone();
            sov.avaa_projekti(&p);
        }
        sov
    }

    fn tallenna_asetukset(&self) {
        if AUTOMAATTI.load(Ordering::Relaxed) {
            return; // itsetesti ei saa muuttaa käyttäjän asetuksia
        }
        if let Ok(t) = serde_json::to_string_pretty(&self.s) {
            let _ = std::fs::write(asetustiedosto(), t);
        }
    }

    fn lokiin(&self, t: &str) {
        crate::loki(t);
    }

    // ------------------------------------------------------------ taustatyöt
    /// Ajaa työn taustalla. Jos edellinen työ on kesken, työ jonotetaan ja aloitetaan sen jälkeen.
    fn taustatyo(&mut self, viesti: &str, f: impl FnOnce(Sender<GuiViesti>) -> Result<()> + Send + 'static) {
        if self.tyo_kaynnissa {
            self.tyojono.push_back((viesti.to_string(), Box::new(f)));
            return;
        }
        self.aloita_tyo(viesti.to_string(), Box::new(f));
    }

    fn aloita_tyo(&mut self, viesti: String, f: Tyo) {
        let viesti = viesti.as_str();
        self.tyo_kaynnissa = true;
        *self.tila.lock().unwrap() = (viesti.to_string(), 0.0);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tulos = f(tx.clone());
            let _ = tx.send(GuiViesti::TyoValmis(tulos.err().map(|e| format!("{e:#}"))));
        });
    }

    fn kasittele_viestit(&mut self, ctx: &egui::Context) {
        while let Ok(v) = self.rx.try_recv() {
            match v {
                GuiViesti::Kehys(video, f, k) => {
                    // myöhässä saapuva kehys edellisestä projektista ohitetaan
                    if self.e_tiedot.as_ref().map(|t| t.polku == video).unwrap_or(false) {
                        self.kehys_saapui(ctx, f, k);
                    }
                }
                GuiViesti::Ehdotukset(f, lista, valmis) => {
                    if Some(f) == Some(self.kohta) {
                        self.ehdotus_kehys = Some(f);
                        for (i, r, k, virhe) in lista {
                            let kirjain = KIRJAIMET[i].to_string();
                            match (r, k) {
                                (Some(r), Some(k)) => {
                                    let tex = tekstuuri(ctx, &format!("ehdotus{kirjain}"), &k);
                                    self.ehdotukset.insert(kirjain.clone(), Ehdotus { resepti: r, kuva: k, tex });
                                    self.virheet.remove(&kirjain);
                                }
                                _ => {
                                    self.ehdotukset.remove(&kirjain);
                                    self.virheet.insert(kirjain, virhe);
                                }
                            }
                        }
                        if valmis {
                            self.paivita_f(ctx);
                            if self.s.nako_suosittele {
                                self.pyyda_suositus();
                            }
                        }
                    }
                }
                GuiViesti::Suositus(f, tulos) => {
                    self.suositus_kaynnissa = None;
                    if let Err(e) = &tulos {
                        self.lokiin(&format!("Vision model: {e}"));
                    }
                    if let Ok(s) = &tulos {
                        self.lokiin(&format!("Vision model ({}, {:.1} s) recommends for frame {f}: {} (runner-up {}) – {}",
                                             s.malli, s.aika_s, s.paras, s.toinen, s.perustelu));
                        // valitse automaattisesti, jos kiinnipisteellä ei vielä ole valintaa
                        if self.s.nako_valitse && f == self.kohta {
                            if let Some(i) = self.kp_tassa() {
                                if self.projekti.kiinnipisteet[i].valinta.is_empty() {
                                    let k = s.paras.clone();
                                    self.valitse_ehdotus(&k);
                                }
                            }
                        }
                    }
                    self.suositus = Some((f, tulos));
                }
                GuiViesti::AnalyysiKp(projekti, kp) => {
                    // tallennetaan vain, jos sama projekti on yhä auki (muuten kirjoitetaan suoraan tiedostoon)
                    if projekti == self.s.e_projekti {
                        let f = kp.kehys;
                        self.projekti.kiinnipisteet.retain(|k| k.kehys != f);
                        self.projekti.kiinnipisteet.push(kp);
                        self.tallenna_projekti();
                        if f == self.kohta {
                            self.valitun_tex = None;
                        }
                    } else if let Ok(mut p) = Projekti::lue(&projekti) {
                        p.kiinnipisteet.retain(|k| k.kehys != kp.kehys);
                        p.kiinnipisteet.push(kp);
                        p.kiinnipisteet.sort_by_key(|k| k.kehys);
                        let _ = p.tallenna(&projekti);
                    }
                }
                GuiViesti::AnalyysiValmis(t) => {
                    self.analyysi_ajo = None;
                    match t {
                        Ok(y) => {
                            self.lokiin(&y);
                            *self.tila.lock().unwrap() = (y, 1.0);
                        }
                        Err(e) => {
                            self.lokiin(&format!("Scene analysis: {e}"));
                            self.tila.lock().unwrap().0 = format!("Scene analysis: {e}");
                        }
                    }
                }
                GuiViesti::Nakomallit(t) => {
                    self.nakomallit_haettu = true;
                    match t {
                        Ok(m) => self.nakomallit = m,
                        Err(e) => self.lokiin(&format!("Vision models: {e}")),
                    }
                }
                GuiViesti::Laatu(t) => {
                    self.laatu_ajo = None;
                    match t {
                        Ok(r) => {
                            *self.tila.lock().unwrap() = ("Quality report ready.".into(), 1.0);
                            self.laatu_tulos = Some(r);
                        }
                        Err(e) => {
                            self.lokiin(&format!("Quality report: {e}"));
                            self.tila.lock().unwrap().0 = format!("Quality report: {e}");
                        }
                    }
                }
                GuiViesti::VEsikatselu(k, t) => {
                    self.v_tex = Some(tekstuuri(ctx, "v_esik", &k));
                    self.v_teksti = t;
                }
                GuiViesti::Automaattiset(kehykset, kohtauksia) => self.lisaa_automaattiset(kehykset, kohtauksia),
                GuiViesti::AjoValmis(t) => {
                    self.ajo = None;
                    let esik = self.esikatselu_odottaa.take();
                    match t {
                        Ok(polku) => {
                            *self.tila.lock().unwrap() = ("Done.".into(), 1.0);
                            match esik {
                                Some((ulos, lahde, alku, kesto)) => self.avaa_soitin(&ulos, &lahde, alku, kesto),
                                None => self.viimeisin_tulos = polku,
                            }
                        }
                        Err(e) => {
                            self.lokiin(&format!("ERROR: {e}"));
                            self.tila.lock().unwrap().0 = format!("Stopped: {e}");
                        }
                    }
                }
                GuiViesti::TyoValmis(virhe) => {
                    self.tyo_kaynnissa = false;
                    match virhe {
                        Some(e) => {
                            self.lokiin(&format!("ERROR: {e}"));
                            self.tila.lock().unwrap().0 = format!("Error: {e}");
                        }
                        None => self.tila.lock().unwrap().0 = "Done.".into(),
                    }
                    if let Some((viesti, f)) = self.tyojono.pop_front() {
                        self.aloita_tyo(viesti, f);
                    } else if self.ehdotukset_jonossa {
                        self.luo_ehdotukset();
                    }
                }
                GuiViesti::LisaosaValmis => {
                    self.lisaosa_ajo = None;
                    self.ollama = None;
                    self.nakomallit_haettu = false;
                }
                GuiViesti::OllamaTila(t) => {
                    self.ollama_haku = false;
                    self.ollama = Some(t);
                }
            }
        }
    }

    // ------------------------------------------------------------ ajo
    fn yhteiset(&self, tila: &str) -> Asetukset {
        let s = &self.s;
        let mut luokat = Vec::new();
        for (k, n) in [(s.lukitus_iho, "iho"), (s.lukitus_taivas, "taivas"), (s.lukitus_kasvit, "kasvit")] {
            if k {
                luokat.push(n.to_string());
            }
        }
        Asetukset {
            tila: tila.into(),
            laite: s.laite.clone(),
            tyoleveys: s.tyoleveys,
            ikkuna: s.ikkuna.max(1),
            virtaus: s.virtaus,
            tasaus_voima: s.tasaus_voima,
            muisti: s.muisti,
            kohtausmuisti: s.kohtausmuisti,
            lukitus: s.lukitus,
            lukitus_luokat: luokat,
            leikkaus_raja: s.leikkaus_raja,
            alku_s: s.nayte_alku,
            kesto_s: s.nayte_kesto,
            koodekki: s.koodekki.clone(),
            crf: s.crf,
            aanilahde: String::new(), // yliajetaan joka tilassa erikseen
            ..Default::default()
        }
    }

    fn rakenna_ajo(&mut self) -> std::result::Result<Option<Asetukset>, String> {
        if !(0.05..=0.95).contains(&self.s.leikkaus_raja) {
            return Err("Scene cut sensitivity must be between 0.05 and 0.95 (default 0.35).".into());
        }
        match self.tila_valilehti {
            Valilehti::Varita => {
                let r = self.s.v_resepti.clone();
                if !Path::new(&self.s.v_video).is_file() {
                    return Err("Choose an existing black-and-white video.".into());
                }
                if r.menetelma != SIIRTO && !mallit::on_ladattu(&r.menetelma) {
                    return Err(format!("Model {} is missing. Install it on the Add-ons tab.", r.menetelma));
                }
                if r.menetelma == SIIRTO && r.referenssi.is_empty() {
                    return Err("Colour transfer needs a reference image.".into());
                }
                if !r.referenssi.is_empty() && !Path::new(&r.referenssi).is_file() {
                    return Err("Reference image not found.".into());
                }
                let mut a = self.yhteiset(if self.s.v_putki { "putki" } else { "vari" });
                a.video = self.s.v_video.clone();
                a.ulos = self.s.v_ulos.clone();
                a.resepti = r;
                a.tayta_malli = self.s.k_tayta_malli.clone();
                a.harmaa_raja = self.s.k_harmaa_raja;
                a.aanilahde = self.s.v_aanilahde.clone();
                a.poista_kopiot = self.s.v_kopiot;
                Ok(Some(a))
            }
            Valilehti::Korjaa => {
                if !Path::new(&self.s.k_video).is_file() {
                    return Err("Choose an existing colorized video.".into());
                }
                if !self.s.k_alkuperainen.is_empty() && !Path::new(&self.s.k_alkuperainen).is_file() {
                    return Err("Original black-and-white video not found.".into());
                }
                let mut a = self.yhteiset("korjaa");
                a.video = self.s.k_video.clone();
                a.alkuperainen = self.s.k_alkuperainen.clone();
                a.ulos = self.s.k_ulos.clone();
                a.tayta_harmaat = self.s.k_taytto;
                a.tayta_malli = self.s.k_tayta_malli.clone();
                a.harmaa_raja = self.s.k_harmaa_raja;
                a.aanilahde = self.s.k_aanilahde.clone();
                Ok(Some(a))
            }
            _ => {
                if self.e_tiedot.is_none() || self.s.e_projekti.is_empty() {
                    return Err("First create an Exemplar project ('New project…') and choose a black-and-white video.".into());
                }
                if self.projekti.kiinnipisteet.is_empty() {
                    if kysy("No keyframes", "The project has no keyframes yet.\n\nMark them yourself on the timeline ('+ Mark this frame'), \
                             or add them automatically to the longest scenes now.\n\nAdd them automatically?") {
                        self.automaattiset();
                    }
                    return Ok(None);
                }
                let ilman = self.projekti.kiinnipisteet.iter().filter(|p| p.valinta.is_empty()).count();
                if ilman > 0 && !kysy("Render", &format!("{ilman} keyframe(s) have no chosen version (A: DDColor will be used). Continue?")) {
                    return Ok(None);
                }
                for p in &self.projekti.kiinnipisteet {
                    for m in p.resepti.mallit() {
                        if !mallit::on_ladattu(&m) {
                            return Err(format!("Model {m} is missing. Install it on the Add-ons tab."));
                        }
                    }
                }
                self.tallenna_projekti();
                let mut a = self.yhteiset("exemplar");
                a.projekti = self.s.e_projekti.clone();
                a.video = self.projekti.video.clone();
                a.ulos = self.s.e_ulos.clone();
                a.aanilahde = self.s.e_aanilahde.clone();
                Ok(Some(a))
            }
        }
    }

    fn aloita(&mut self) {
        if self.ajo.is_some() {
            return;
        }
        let a = match self.rakenna_ajo() {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => {
                virhe(&e);
                return;
            }
        };
        self.esikatselu_odottaa = None;
        self.aloita_ajo(a);
    }

    /// Väritä/korjaa/tuota satunnainen X sekunnin pätkä nykyisillä asetuksilla ja toista se soittimessa.
    fn aloita_esikatselu(&mut self) {
        if self.ajo.is_some() {
            return;
        }
        let mut a = match self.rakenna_ajo() {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => {
                virhe(&e);
                return;
            }
        };
        let t = match Tiedot::lue(&a.video) {
            Ok(t) => t,
            Err(e) => return virhe(&format!("{e}")),
        };
        let kesto = self.s.esik_kesto.clamp(1.0, 120.0).min(t.kesto);
        let alku = satunnainen(0.0, (t.kesto - kesto - 0.1).max(0.0));
        let kansio = sovelluskansio().join("previews");
        let _ = std::fs::create_dir_all(&kansio);
        // vanhat esikatselut pois (pidetään viimeiset 5)
        if let Ok(d) = std::fs::read_dir(&kansio) {
            let mut v: Vec<_> = d.flatten().filter(|e| e.path().extension().map(|x| x == "mkv").unwrap_or(false)).collect();
            v.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            for e in v.iter().take(v.len().saturating_sub(4)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let leima = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        a.alku_s = alku;
        a.kesto_s = kesto;
        a.ulos = kansio.join(format!("{}_{}_{leima}.mkv", a.tila, aikateksti(alku)[..7].replace(':', "-"))).to_string_lossy().into();
        self.lokiin(&format!("Random preview: {} – {} ({kesto:.0} s)", aikateksti(alku), aikateksti(alku + kesto)));
        self.esikatselu_odottaa = Some((a.ulos.clone(), a.video.clone(), alku, kesto));
        self.aloita_ajo(a);
    }

    fn avaa_soitin(&mut self, ulos: &str, lahde: &str, alku: f64, kesto: f64) {
        let ctx = self.ctx.clone();
        self.soitin = Some(Soitin::uusi(&ctx, ulos, lahde, alku, kesto, self.s.esik_vertaa));
    }

    fn nayta_soitin(&mut self, ctx: &egui::Context) {
        let Some(s) = self.soitin.as_mut() else { return };
        while let Ok(k) = s.rx.try_recv() {
            match &mut s.tex {
                Some(t) => t.set(egui::ColorImage::from_rgb([k.w, k.h], &k.rgb()), egui::TextureOptions::LINEAR),
                None => s.tex = Some(tekstuuri(ctx, "soitin", &k)),
            }
        }
        let mut auki = true;
        let mut uusi = false;
        let mut vertaa_vaihto = false;
        let nimi = Path::new(&s.ulos).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        egui::Window::new("Random preview").open(&mut auki).resizable(true).default_width(980.0).show(ctx, |ui| {
            ui.label(format!("{} – {} ({:.0} s){}   {nimi}", aikateksti(s.alku), aikateksti(s.alku + s.kesto), s.kesto,
                             if s.vertaa { "   left: source, right: result" } else { "" }));
            match &s.tex {
                Some(t) => {
                    let [w, h] = t.size();
                    let lev = ui.available_width().min(w as f32 * 1.6);
                    ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::new(lev, lev * h as f32 / w as f32)));
                }
                None => {
                    ui.spinner();
                }
            }
            ui.horizontal(|ui| {
                let tauolla = s.tauko.load(Ordering::Relaxed);
                if ui.button(if tauolla { "▶ Play" } else { "⏸ Pause" }).clicked() {
                    s.tauko.store(!tauolla, Ordering::Relaxed);
                }
                if ui.button("🎲 New random spot").clicked() {
                    uusi = true;
                }
                if ui.checkbox(&mut self.s.esik_vertaa, "Compare with source").changed() {
                    vertaa_vaihto = true;
                }
                if ui.button("Open in player (with sound)").clicked() && !AUTOMAATTI.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("explorer").arg(&s.ulos).spawn();
                }
                ui.label(egui::RichText::new("plays in a loop").color(Color32::GRAY));
            });
        });
        if vertaa_vaihto {
            let (u, l, a, k) = (s.ulos.clone(), s.lahde.clone(), s.alku, s.kesto);
            self.avaa_soitin(&u, &l, a, k);
        }
        if !auki {
            self.soitin = None;
        }
        if uusi && self.ajo.is_none() {
            self.soitin = None;
            self.aloita_esikatselu();
        }
    }

    fn aloita_ajo(&mut self, a: Asetukset) {
        self.tallenna_asetukset();
        let seis = Arc::new(AtomicBool::new(false));
        self.ajo = Some(seis.clone());
        *self.tila.lock().unwrap() = ("Running…".into(), 0.0);
        self.lokiin(&format!("\n===== {} =====", match a.tila.as_str() { "putki" => "Colorize → check → repair", "vari" => "Colorize", "korjaa" => "Repair", _ => "Exemplar render" }));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tulos = crate::tuotanto::suorita(a, seis);
            let _ = tx.send(GuiViesti::AjoValmis(tulos.map_err(|e| format!("{e:#}"))));
        });
    }

    fn pysayta(&mut self) {
        if let Some(s) = &self.ajo {
            s.store(true, Ordering::Relaxed);
            self.lokiin("Stopping…");
        }
    }

    // ------------------------------------------------------------ väritä
    fn v_esikatsele(&mut self) {
        let tiedot = match Tiedot::lue(&self.s.v_video) {
            Ok(t) => t,
            Err(e) => return virhe(&format!("{e}")),
        };
        let Some(s) = aika_sekunneiksi(&self.s.v_aika) else { return virhe("Enter the time as h:m:s, m:s or seconds.") };
        let kehys = tiedot.kehys(s).min(tiedot.n - 1);
        let r = self.s.v_resepti.clone();
        let (laite, refit) = (self.s.laite.clone(), self.refit.clone());
        self.taustatyo("Computing preview…", move |tx| {
            let k = hae_kehys(&tiedot, kehys, Some(tiedot.koko_enintaan(ESIKATSELU_LEVEYS)))?;
            let (l, ab) = aja_resepti(&r, &k, &mut HashMap::new(), &laite, &refit)?;
            let mv = Bgr::rgb_kuvasta(k.w, k.h, &k.harmaa().iter().flat_map(|&g| [g, g, g]).collect::<Vec<u8>>());
            let kuva = Bgr::vierekkain(&[&mv, &lab_bgr(&l, &ab)]);
            let _ = tx.send(GuiViesti::VEsikatselu(kuva, format!("{}  –  {}", aikateksti(tiedot.aika(kehys)), r.kuvaus())));
            Ok(())
        });
    }

    fn nayta_varita(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("v_tied").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            if tiedostorivi(ui, "Black-and-white video:", &mut self.s.v_video, || pick("Black-and-white video", &[("Video", VIDEOT)], "")) {
                self.s.v_ulos = ehdota_ulos(&self.s.v_video, "_colorized");
            }
            let ulos = self.s.v_ulos.clone();
            tiedostorivi(ui, "Output:", &mut self.s.v_ulos, || tallenna_nimella("Output", &[("MKV", &["mkv"]), ("MP4", &["mp4"])], &ulos));
            tiedostorivi(ui, "Audio source (optional):", &mut self.s.v_aanilahde, || pick("Audio source", &[("Video/Audio", VIDEOT)], ""));
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.s.v_putki, egui::RichText::new("Check and repair automatically (recommended)").strong());
            ui.label(egui::RichText::new(
                "colorize → frame-by-frame check → repair: grey patches and frames are filled, colours are compared \
                 with previous frames. Fill model and grey threshold: Repair tab.").color(Color32::GRAY));
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.s.v_kopiot, "Drop duplicate frames (true frame rate)");
            ui.label(egui::RichText::new(
                "e.g. in a video converted from 25 to 30 fps every 6th frame is a copy: copies are dropped and the result \
                 is written at 25 fps (duration and sound unchanged). Occasional copies reuse the previous frame's colour.").color(Color32::GRAY));
        });
        ui.horizontal(|ui| {
            ui.label("Method:");
            let m = &mut self.s.v_resepti.menetelma;
            egui::ComboBox::from_id_salt("v_menetelma").width(220.0).selected_text(m.as_str()).show_ui(ui, |ui| {
                for t in mallit::valittavat() {
                    ui.selectable_value(m, t.to_string(), t);
                }
                ui.selectable_value(m, SIIRTO.to_string(), SIIRTO);
            });
            let puuttuu = *m != SIIRTO && !mallit::on_ladattu(m);
            ui.label(egui::RichText::new(format!("{}{}", menetelman_nimi(m), if puuttuu { "   – NOT INSTALLED (Add-ons tab)" } else { "" }))
                .color(if puuttuu { Color32::from_rgb(220, 90, 40) } else { Color32::GRAY }));
        });
        egui::Grid::new("v_ref").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            tiedostorivi(ui, "Reference image (optional):", &mut self.s.v_resepti.referenssi, || pick("Reference image", &[("Image", KUVAT)], ""));
        });
        ui.horizontal(|ui| {
            let r = &mut self.s.v_resepti;
            ui.add(egui::Slider::new(&mut r.ref_voima, 0.0..=1.0).text("Reference strength"));
            ui.add(egui::Slider::new(&mut r.h, -180.0..=180.0).text("Hue H°"));
            ui.add(egui::Slider::new(&mut r.s, 0.0..=3.0).text("Saturation S"));
            ui.add(egui::Slider::new(&mut r.v, 0.6..=1.4).text("Brightness V"));
            if ui.add_enabled(r.h != 0.0 || r.s != 1.0 || r.v != 1.0, egui::Button::new("Reset H/S/V")).clicked() {
                (r.h, r.s, r.v) = (0.0, 1.0, 1.0);
            }
        });
        if let Some(v) = self.s.v_resepti.saatovaroitus() {
            ui.label(egui::RichText::new(format!("⚠ {v}. Natural colours: H 0, S 1.0 (\"Reset H/S/V\").")).color(Color32::from_rgb(230, 120, 30)).strong());
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Preview position (h:m:s or s):");
            ui.add_sized([90.0, 22.0], egui::TextEdit::singleline(&mut self.s.v_aika));
            if ui.add_enabled(!self.tyo_kaynnissa, egui::Button::new("Preview with these settings")).clicked() {
                self.v_esikatsele();
            }
            ui.label(egui::RichText::new("Wheel = zoom, drag = pan, double-click = reset").color(Color32::GRAY));
        });
        let koko = ui.available_size();
        self.v_nakyma.piirra(ui, self.v_tex.as_ref(), &self.v_teksti, koko);
    }

    fn nayta_korjaa(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("k_tied").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            if tiedostorivi(ui, "Colorized video:", &mut self.s.k_video, || pick("Colorized video", &[("Video", VIDEOT)], "")) {
                self.s.k_ulos = ehdota_ulos(&self.s.k_video, "_repaired");
            }
            tiedostorivi(ui, "Original black-and-white (optional):", &mut self.s.k_alkuperainen, || pick("Original", &[("Video", VIDEOT)], ""));
            let ulos = self.s.k_ulos.clone();
            tiedostorivi(ui, "Output:", &mut self.s.k_ulos, || tallenna_nimella("Output", &[("MKV", &["mkv"]), ("MP4", &["mp4"])], &ulos));
            tiedostorivi(ui, "Audio source (optional):", &mut self.s.k_aanilahde, || pick("Audio source", &[("Video/Audio", VIDEOT)], ""));
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.s.k_taytto, "Recolour grey frames and uncoloured patches with model:");
            egui::ComboBox::from_id_salt("k_malli").width(200.0).selected_text(self.s.k_tayta_malli.as_str()).show_ui(ui, |ui| {
                for t in mallit::MALLIT {
                    ui.selectable_value(&mut self.s.k_tayta_malli, t.nimi.to_string(), t.nimi);
                }
            });
        });
        ui.add(egui::Slider::new(&mut self.s.k_harmaa_raja, 1.0..=12.0).text("Grey frame threshold (mean chroma)"));
        ui.add_space(6.0);
        ui.add_space(8.0);
        ui.label(egui::RichText::new(
            "Repair does not change colours dramatically: it removes flicker (optical flow + temporal smoothing),\n\
             locks the hue of skin, sky and vegetation within a scene and fills uncoloured areas.\n\
             If the original black-and-white video is given, brightness and sharpness are taken from it.\n\
             Settings: 'Stabilization & output' tab.").color(Color32::GRAY));
    }

    fn nayta_asetukset(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.s;
        ui.heading("Flicker removal");
        ui.add(egui::Slider::new(&mut s.ikkuna, 1..=15).text("Temporal smoothing window (frames, 1 = off)"));
        ui.add(egui::Slider::new(&mut s.tasaus_voima, 0.0..=1.0).text("Smoothing strength"));
        ui.add(egui::Slider::new(&mut s.muisti, 0.0..=0.98).text("Colour memory (0 = off, default 0.98)"));
        ui.label(egui::RichText::new(
            "Colour follows motion through the whole scene: an object keeps the colour the model mostly suggested \
             for it and does not switch mid-scene. 0.98 ≈ average of 50 frames, 0.9 ≈ 10 frames.").color(Color32::GRAY));
        ui.add_enabled_ui(crate::piirteet::on_ladattu(), |ui| {
            ui.checkbox(&mut s.kohtausmuisti, "Scene memory across cuts (DINOv2)");
        });
        ui.label(egui::RichText::new(if crate::piirteet::on_ladattu() {
            "The same object (coat, wall, face) keeps its colour when the camera angle changes and returns. Requires colour memory."
        } else {
            "Install dinov2_small on the Add-ons tab."
        }).color(Color32::GRAY));
        ui.checkbox(&mut s.virtaus, "Optical flow (colours follow motion – recommended)");
        ui.add(egui::Slider::new(&mut s.leikkaus_raja, 0.05..=0.95).text("Scene cut sensitivity (lower = more sensitive, default 0.35)"));
        ui.separator();
        ui.heading("Colour lock");
        ui.add(egui::Slider::new(&mut s.lukitus, 0.0..=1.0).text("Strength"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.lukitus_iho, "Skin");
            ui.checkbox(&mut s.lukitus_taivas, "Sky");
            ui.checkbox(&mut s.lukitus_kasvit, "Vegetation");
        });
        ui.label(egui::RichText::new("Keeps the mean hue of the selected areas constant within a scene.").color(Color32::GRAY));
        ui.separator();
        ui.heading("Output");
        ui.horizontal(|ui| {
            ui.label("Compute device:");
            for (v, n) in [("auto", "Automatic"), ("gpu", "Graphics card (DirectML)"), ("cpu", "CPU")] {
                ui.radio_value(&mut s.laite, v.to_string(), n);
            }
        });
        ui.add(egui::Slider::new(&mut s.tyoleveys, 320..=1920).text("Colour processing width (px)"));
        ui.horizontal(|ui| {
            ui.label("Codec:");
            for k in ["x264", "x265", "nvenc"] {
                ui.radio_value(&mut s.koodekki, k.to_string(), k);
            }
        });
        ui.add(egui::Slider::new(&mut s.crf, 0..=35).text("Quality CRF (lower = better)"));
        ui.horizontal(|ui| {
            ui.label("Sample: start (s)");
            ui.add(egui::DragValue::new(&mut s.nayte_alku).range(0.0..=100000.0).speed(1.0));
            ui.label("duration (s, 0 = whole video)");
            ui.add(egui::DragValue::new(&mut s.nayte_kesto).range(0.0..=100000.0).speed(1.0));
        });
        ui.add_space(10.0);
        if ui.button("Restore defaults (paths are kept)").clicked() {
            let d = Tallennettavat::default();
            let polut = (s.v_video.clone(), s.v_ulos.clone(), s.k_video.clone(), s.k_alkuperainen.clone(), s.k_ulos.clone(), s.e_projekti.clone(), s.e_ulos.clone());
            *s = d;
            (s.v_video, s.v_ulos, s.k_video, s.k_alkuperainen, s.k_ulos, s.e_projekti, s.e_ulos) = polut;
            crate::loki("Settings restored to defaults (paths kept).");
        }
    }

    fn nayta_lisaosat(&mut self, ui: &mut egui::Ui) {
        let vihrea = Color32::from_rgb(60, 160, 60);
        let oranssi = Color32::from_rgb(220, 90, 40);
        let vapaa = self.lisaosa_ajo.is_none();
        ui.heading("Add-ons");
        ui.label(format!("Everything is installed into the application folder {}. The only separately installed program is Ollama (Assistant only).",
                         sovelluskansio().display()));
        if crate::lisaosat::puuttuu_pakollisia() {
            ui.add_space(4.0);
            if !mallit::on_ladattu("ddcolor_suomi") {
                ui.label(egui::RichText::new("The colorization model models/ddcolor_suomi/ddcolor_suomi.onnx is missing – reinstall BW2Color Movie.")
                    .color(oranssi).strong());
            }
            if crate::video::ohjelma("ffmpeg").is_err() || crate::video::ohjelma("ffprobe").is_err() {
                ui.label(egui::RichText::new("Colorizing needs FFmpeg.").color(oranssi).strong());
                let teksti = format!("⬇ Install FFmpeg (~{} MB)", crate::lisaosat::FFMPEG_KOKO_MT);
                if ui.add_enabled(vapaa, egui::Button::new(egui::RichText::new(teksti).strong())).clicked() {
                    self.asenna(vec!["ffmpeg".to_string()]);
                }
            }
        }
        if let Some((kuvaus, seis)) = &self.lisaosa_ajo {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Installing: {kuvaus}"));
                if ui.button("Cancel").clicked() {
                    seis.store(true, Ordering::Relaxed);
                }
            });
        }
        ui.add_space(8.0);
        let mut asennettavat = Vec::new();
        let mut poistettava = None;
        let mut tuotava = None;
        egui::Grid::new("lisaosat").striped(true).num_columns(6).spacing([14.0, 6.0]).show(ui, |ui| {
            for o in ["Component", "Description", "Size", "Licence", "Status", ""] {
                ui.strong(o);
            }
            ui.end_row();
            for o in crate::lisaosat::osat() {
                ui.label(if o.pakollinen { format!("{} *", o.nimi) } else { o.nimi.clone() });
                ui.label(&o.kuvaus);
                ui.label(format!("{} MB", o.koko_mt));
                ui.label(egui::RichText::new(o.lisenssi).color(Color32::GRAY));
                match &o.asennettu {
                    Some(p) => ui.label(egui::RichText::new("✔ installed").color(vihrea)).on_hover_text(p),
                    None => ui.label(egui::RichText::new("missing").color(oranssi)),
                };
                ui.horizontal(|ui| {
                    // järjestelmän PATHista löytyvä ffmpeg kelpaa, mutta oman voi silti asentaa
                    let oma = o.asennettu.as_deref().map(|p| !p.ends_with("(system PATH)")).unwrap_or(false);
                    if !oma && matches!(o.lahde, crate::lisaosat::Lahde::Verkko) && ui.add_enabled(vapaa, egui::Button::new("Download")).clicked() {
                        asennettavat.push(o.tunnus.clone());
                    }
                    if o.tunnus.starts_with("malli:") && ui.add_enabled(vapaa, egui::Button::new("Import from file…"))
                        .on_hover_text("Copies an .onnx model, e.g. from another installation (Paper and Tiny cannot be downloaded)")
                        .clicked()
                    {
                        tuotava = Some(o.tunnus.clone());
                    }
                    if oma && ui.add_enabled(vapaa, egui::Button::new("Remove")).clicked() {
                        poistettava = Some((o.tunnus.clone(), o.nimi.clone()));
                    }
                });
                ui.end_row();
            }
        });
        ui.label(egui::RichText::new("* needed for colorizing (one DDColor model is enough)").color(Color32::GRAY));
        if !asennettavat.is_empty() {
            self.asenna(asennettavat);
        }
        if let Some((tunnus, nimi)) = poistettava {
            if kysy("Remove add-on", &format!("Remove {nimi}?")) {
                let r = match tunnus.strip_prefix("malli:") {
                    Some(m) => mallit::poista(m),
                    None if tunnus.starts_with("piirteet:") => crate::piirteet::poista(),
                    None => crate::lisaosat::poista_ffmpeg(),
                };
                match r {
                    Ok(()) => crate::loki(&format!("Removed: {nimi}")),
                    Err(e) => virhe(&format!("{e:#}")),
                }
            }
        }
        if let Some(tunnus) = tuotava {
            let nimi = tunnus.trim_start_matches("malli:").to_string();
            if let Some(f) = pick(&format!("Choose {nimi}.onnx"), &[("ONNX model", &["onnx"])], "") {
                self.aja_lisaosatyo(format!("{nimi} from file"), move |_| {
                    mallit::tuo(&nimi, Path::new(&f)).map(|p| crate::loki(&format!("Imported: {}", p.display())))
                });
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Open application folder").clicked() {
                let _ = std::process::Command::new("explorer").arg(sovelluskansio()).spawn();
            }
            if ui.button("Licences").on_hover_text("LICENSES.txt: licences of all components").clicked() {
                let _ = std::process::Command::new("notepad").arg(sovelluskansio().join("LICENSES.txt")).spawn();
            }
        });

        ui.add_space(14.0);
        ui.heading("Vision model (Assistant, optional)");
        if self.ollama.is_none() && !self.ollama_haku && vapaa {
            self.ollama_haku = true;
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(GuiViesti::OllamaTila(crate::lisaosat::ollama_tila()));
            });
        }
        let Some(t) = &self.ollama else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking Ollama…");
            });
            return;
        };
        if !t.asennettu {
            ui.label("Ollama is not installed. The Assistant (recommendations, scene analysis, quality report) needs it; colorizing works without it.");
            ui.horizontal(|ui| {
                if ui.button("Open ollama.com/download").clicked() {
                    let _ = std::process::Command::new("explorer").arg("https://ollama.com/download").spawn();
                }
                if ui.button("Check again").clicked() {
                    self.ollama = None;
                }
            });
            return;
        }
        if let Some(e) = &t.virhe {
            ui.label(egui::RichText::new(format!("Ollama is not responding: {e}")).color(oranssi));
        }
        ui.label(format!("Ollama {}. Vision models: {}", if t.kaynnissa { "running" } else { "installed" },
                         if t.nakomallit.is_empty() { "none".to_string() } else { t.nakomallit.len().to_string() }));
        let mut veto = None;
        let mut poisto = None;
        for m in &t.nakomallit {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("✔ {m}")).color(vihrea));
                if ui.add_enabled(vapaa, egui::Button::new("Remove")).clicked() {
                    poisto = Some(m.clone());
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Download model:");
            ui.add(egui::TextEdit::singleline(&mut self.nako_veto).desired_width(180.0));
            if ui.add_enabled(vapaa && !self.nako_veto.trim().is_empty(), egui::Button::new("Download into Ollama"))
                .on_hover_text("Recommended: qwen2.5vl:7b (~6 GB, Qwen licence). An interrupted download resumes where it left off.")
                .clicked()
            {
                veto = Some(self.nako_veto.trim().to_string());
            }
        });
        if let Some(m) = veto {
            self.asenna(vec![format!("nako:{m}")]);
        }
        if let Some(m) = poisto {
            if kysy("Remove vision model", &format!("Remove {m} from Ollama?")) {
                self.aja_lisaosatyo(format!("removing {m}"), move |_| crate::nako::poista_malli(&m));
            }
        }
    }

    fn hae_nakomallit(&mut self) {
        self.nakomallit_haettu = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(GuiViesti::Nakomallit(crate::nako::nakomallit().map_err(|e| format!("{e:#}"))));
        });
    }

    fn aloita_laatu(&mut self) {
        if self.laatu_ajo.is_some() {
            return;
        }
        let video = self.s.laatu_video.trim().to_string();
        if !Path::new(&video).is_file() {
            return virhe("Choose the (colorized) video to check.");
        }
        let seis = Arc::new(AtomicBool::new(false));
        self.laatu_ajo = Some(seis.clone());
        self.laatu_tulos = None;
        let (n, malli, tx) = (self.s.nako_naytteita, self.s.nako_malli.clone(), self.tx.clone());
        *self.tila.lock().unwrap() = ("Quality report running…".into(), 0.0);
        std::thread::spawn(move || {
            let t = crate::nako::laaturaportti(&video, n, &malli, &seis).map(|r| (r.html, r.havainnot)).map_err(|e| format!("{e:#}"));
            let _ = tx.send(GuiViesti::Laatu(t));
        });
    }

    fn nayta_avustaja(&mut self, ui: &mut egui::Ui) {
        if !self.nakomallit_haettu {
            self.hae_nakomallit();
        }
        ui.heading("Vision model assistant (Ollama, local)");
        let ollama = crate::nako::ollama_exe();
        ui.horizontal(|ui| {
            match &ollama {
                Some(p) => ui.label(egui::RichText::new(format!("Ollama: {}", p.display())).color(Color32::from_rgb(60, 160, 60))),
                None => ui.label(egui::RichText::new("Ollama not found – install it from ollama.com.").color(Color32::from_rgb(220, 90, 40))),
            };
            if ui.button("Refresh model list").clicked() {
                self.hae_nakomallit();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Vision model:");
            egui::ComboBox::from_id_salt("nako_malli").width(220.0).selected_text(self.s.nako_malli.as_str()).show_ui(ui, |ui| {
                for m in &self.nakomallit {
                    ui.selectable_value(&mut self.s.nako_malli, m.clone(), m);
                }
            });
            ui.label(egui::RichText::new(format!("{} vision model(s) installed. Recommended: {} (compares several images reliably).",
                                                 self.nakomallit.len(), crate::nako::OLETUSMALLI)).color(Color32::GRAY));
        });
        ui.label(egui::RichText::new("The vision model does not colorize: it looks at the images and helps you choose and check. \
                                       Everything runs on your own computer.").color(Color32::GRAY));
        ui.separator();
        ui.strong("Exemplar: recommending a suggestion");
        ui.checkbox(&mut self.s.nako_suosittele, "Recommend the best suggestion A–E automatically once suggestions are computed");
        ui.add_enabled(self.s.nako_suosittele, egui::Checkbox::new(&mut self.s.nako_valitse,
                                                                    "Pick the recommendation automatically if the keyframe has no choice yet"));
        ui.separator();
        ui.strong("Exemplar: scene analysis");
        ui.label(egui::RichText::new(
            "A keyframe in the middle of every scene, suggestions A–E (B and E only with a reference image), the vision model's choice \
             and the scene type (day/night, interior/exterior, close-up). All suggestions are computed first, then the vision model chooses – \
             the colorization models and the vision model do not fit in GPU memory at the same time. Estimate: about 3–6 s per scene.").color(Color32::GRAY));
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.s.analyysi_min_kesto, 0.0..=10.0).text("s: shorter scenes take their colours from neighbours"));
        });
        ui.horizontal(|ui| {
            ui.label("At most");
            ui.add(egui::DragValue::new(&mut self.s.analyysi_enintaan).range(0..=5000));
            ui.label("scenes per run (0 = all)");
            if self.analyysi_ajo.is_some() {
                if ui.button("■ Stop analysis").clicked() {
                    if let Some(s) = &self.analyysi_ajo {
                        s.store(true, Ordering::Relaxed);
                    }
                }
            } else if ui.add_enabled(self.e_tiedot.is_some(), egui::Button::new("▶ Analyse scenes (Exemplar project)")).clicked() {
                self.aloita_analyysi();
            }
        });
        ui.separator();
        ui.strong("Quality report of a finished video");
        egui::Grid::new("laatu").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            tiedostorivi(ui, "Video:", &mut self.s.laatu_video, || pick("Video to check", &[("Video", VIDEOT)], ""));
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.viimeisin_tulos.is_empty(), egui::Button::new("Use the latest result")).clicked() {
                self.s.laatu_video = self.viimeisin_tulos.clone();
            }
            ui.add(egui::Slider::new(&mut self.s.nako_naytteita, 10..=300).text("samples (evenly across the video)"));
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(self.laatu_ajo.is_none(), egui::Button::new(egui::RichText::new("▶ Create quality report").strong())).clicked() {
                self.aloita_laatu();
            }
            if ui.add_enabled(self.laatu_ajo.is_some(), egui::Button::new("■ Stop")).clicked() {
                if let Some(s) = &self.laatu_ajo {
                    s.store(true, Ordering::Relaxed);
                }
            }
        });
        ui.label(egui::RichText::new("Checks: unnatural skin, uncoloured frames and patches, overall colour cast, \
                                       oversaturation. Vision model findings are verified by measurements; a mere suspicion = 'check'.")
            .color(Color32::GRAY));
        if let Some((html, havainnot)) = &self.laatu_tulos {
            let ongelmat: Vec<&crate::nako::Havainto> = havainnot.iter().filter(|h| !h.kunnossa).collect();
            let tarkista = havainnot.iter().filter(|h| h.kunnossa && h.vakavuus == 1).count();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong(format!("{} samples: {} problems, {tarkista} to check", havainnot.len(), ongelmat.len()));
                if ui.button("Open report").clicked() && !AUTOMAATTI.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("explorer").arg(html).spawn();
                }
            });
            egui::ScrollArea::vertical().id_salt("laatu_lista").max_height(220.0).show(ui, |ui| {
                for h in havainnot.iter().filter(|h| h.vakavuus >= 1) {
                    let vari = match h.vakavuus { 3 => Color32::from_rgb(210, 40, 40), 2 => Color32::from_rgb(224, 112, 32), _ => Color32::from_rgb(200, 160, 0) };
                    ui.label(egui::RichText::new(format!("{}  {}  {}  {}", &h.aika[..h.aika.len() - 4],
                                                         if h.vakavuus == 1 { "check".into() } else { "●".repeat(h.vakavuus as usize) },
                                                         h.ongelmat.join(", "), h.kuvaus)).color(vari).monospace().size(12.0));
                }
            });
        }
    }

    /// Asentaa lisäosat järjestyksessä taustalla ("ffmpeg", "malli:<nimi>", "nako:<ollama-malli>").
    fn asenna(&mut self, tunnukset: Vec<String>) {
        if tunnukset.is_empty() {
            return;
        }
        let nimi = |t: &str| t.split_once(':').map(|x| x.1.to_string()).unwrap_or_else(|| t.to_string());
        let kuvaus = tunnukset.iter().map(|t| nimi(t)).collect::<Vec<_>>().join(", ");
        self.aja_lisaosatyo(kuvaus, move |seis| {
            let mt = |s: u64| s as f64 / 1048576.0;
            for t in &tunnukset {
                if t == "ffmpeg" {
                    crate::lisaosat::asenna_ffmpeg(&mut |s, k| crate::edistys(&format!("FFmpeg: {:.0} / {:.0} MB", mt(s), mt(k)), s as usize, k.max(1) as usize), seis)?;
                } else if let Some(m) = t.strip_prefix("malli:") {
                    mallit::lataa(m, &mut |s, k| crate::edistys(&format!("{m}: {:.0} / {:.0} MB", mt(s), mt(k)), s as usize, k.max(1) as usize), seis)?;
                } else if t.starts_with("piirteet:") {
                    crate::piirteet::lataa(&mut |s, k| crate::edistys(&format!("DINOv2: {:.0} / {:.0} MB", mt(s), mt(k)), s as usize, k.max(1) as usize), seis)?;
                } else if let Some(m) = t.strip_prefix("nako:") {
                    crate::nako::veda_malli(m, &mut |tila, s, k| {
                        let teksti = if k > 0 { format!("{m}: {tila} {:.0} / {:.0} MB", mt(s), mt(k)) } else { format!("{m}: {tila}") };
                        crate::edistys(&teksti, s as usize, k.max(1) as usize)
                    }, seis)?;
                }
                crate::loki(&format!("Installed: {}", nimi(t)));
            }
            Ok(())
        });
    }

    fn aja_lisaosatyo(&mut self, kuvaus: String, f: impl FnOnce(&AtomicBool) -> anyhow::Result<()> + Send + 'static) {
        if self.lisaosa_ajo.is_some() {
            return;
        }
        let seis = Arc::new(AtomicBool::new(false));
        self.lisaosa_ajo = Some((kuvaus.clone(), seis.clone()));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            match f(&seis) {
                Ok(()) => crate::edistys(&format!("Done: {kuvaus}"), 1, 1),
                Err(e) => crate::loki(&format!("ERROR: {kuvaus}: {e:#}")),
            }
            let _ = tx.send(GuiViesti::LisaosaValmis);
        });
    }
}

// ============================================================================
// Exemplar
// ============================================================================
impl Sovellus {
    fn uusi_projekti(&mut self) {
        let Some(video) = pick("Choose a BLACK-AND-WHITE video", &[("Video", VIDEOT)], "") else { return };
        let p = Path::new(&video);
        let oletus = p.with_file_name(format!("{}.bw2color.json", p.file_stem().unwrap().to_string_lossy()));
        let Some(polku) = tallenna_nimella("Project file", &[("Project", &["json"])], &oletus.to_string_lossy()) else { return };
        self.projekti = Projekti { versio: 1, video: video.clone(), kiinnipisteet: vec![] };
        self.s.e_projekti = polku;
        self.s.e_ulos = ehdota_ulos(&video, "_exemplar");
        self.tallenna_projekti();
        self.lataa_projektin_video();
    }

    fn avaa_projekti(&mut self, polku: &str) {
        match Projekti::lue(polku) {
            Ok(p) => {
                self.projekti = p;
                self.s.e_projekti = polku.to_string();
                if self.s.e_ulos.is_empty() {
                    self.s.e_ulos = ehdota_ulos(&self.projekti.video, "_exemplar");
                }
                self.lataa_projektin_video();
            }
            Err(e) => virhe(&format!("Cannot open the project:\n{e}")),
        }
    }

    fn tallenna_projekti(&mut self) {
        if self.s.e_projekti.is_empty() {
            return;
        }
        self.projekti.kiinnipisteet.sort_by_key(|p| p.kehys);
        if let Err(e) = self.projekti.tallenna(&self.s.e_projekti) {
            self.lokiin(&format!("Saving the project failed: {e}"));
        }
    }

    fn kuvakansio(&self) -> PathBuf {
        let p = Path::new(&self.s.e_projekti);
        let k = p.with_file_name(format!("{}_kuvat", p.file_stem().unwrap_or_default().to_string_lossy()));
        let _ = std::fs::create_dir_all(&k);
        k
    }

    fn lataa_projektin_video(&mut self) {
        match Tiedot::lue(&self.projekti.video) {
            Ok(t) => self.e_tiedot = Some(t),
            Err(e) => {
                self.e_tiedot = None;
                return virhe(&format!("{e}"));
            }
        }
        self.ab_muisti.lock().unwrap().clear();
        self.ehdotukset.clear();
        self.ehdotus_kehys = None;
        let alku = self.projekti.kiinnipisteet.first().map(|p| p.kehys).unwrap_or(0);
        self.siirry(alku);
    }

    fn kp_tassa(&self) -> Option<usize> {
        self.projekti.kiinnipisteet.iter().position(|p| p.kehys == self.kohta)
    }

    fn siirry(&mut self, kehys: usize) {
        let Some(t) = &self.e_tiedot else { return };
        self.kohta = kehys.min(t.n.saturating_sub(1));
        self.e_aika = aikateksti(t.aika(self.kohta));
        if let Some(i) = self.kp_tassa() {
            let kp = &self.projekti.kiinnipisteet[i];
            self.hsv = (kp.resepti.h, kp.resepti.s, kp.resepti.v);
            self.kp_ref_voima = kp.resepti.ref_voima;
            self.f_pohja = if kp.f_pohja.is_empty() { "A".into() } else { kp.f_pohja.clone() };
        }
        self.pyyda_kehys(self.kohta);
    }

    /// Hakee kehyksen taustalla; nopeassa selauksessa vain viimeisin pyyntö haetaan.
    fn pyyda_kehys(&mut self, f: usize) {
        let Some(t) = self.e_tiedot.clone() else { return };
        let mut jono = self.hakupyynto.lock().unwrap();
        *jono = Some((t, f));
        if self.haku_kaynnissa.load(Ordering::SeqCst) {
            return; // käynnissä oleva säie poimii uusimman pyynnön
        }
        self.haku_kaynnissa.store(true, Ordering::SeqCst);
        drop(jono);
        let (pyynto, kaynnissa, tx) = (self.hakupyynto.clone(), self.haku_kaynnissa.clone(), self.tx.clone());
        std::thread::spawn(move || loop {
            let seuraava = {
                let mut j = pyynto.lock().unwrap();
                let s = j.take();
                if s.is_none() {
                    // lopetus lukon alla: pyyntö ei voi jäädä käsittelemättä
                    kaynnissa.store(false, Ordering::SeqCst);
                }
                s
            };
            let Some((t, f)) = seuraava else { break };
            let k = match hae_kehys(&t, f, Some(t.koko_enintaan(ESIKATSELU_LEVEYS))) {
                Ok(k) => Some(k),
                Err(e) => {
                    crate::loki(&format!("Reading frame {f} failed: {e}"));
                    None
                }
            };
            if pyynto.lock().unwrap().is_none() {
                let _ = tx.send(GuiViesti::Kehys(t.polku.clone(), f, k));
            }
        });
    }

    fn kehys_saapui(&mut self, ctx: &egui::Context, f: usize, k: Option<Bgr>) {
        if f != self.kohta {
            return;
        }
        self.kehys_tex = k.as_ref().map(|k| tekstuuri(ctx, "kehys", k));
        if let (Some(k), Some(t)) = (&k, &self.e_tiedot) {
            if self.tarkistettu_video != t.polku {
                self.tarkistettu_video = t.polku.clone();
                let kr = kroma(&bgr_lab(k).ab).keskiarvo();
                self.e_varoitus = if kr > 5.0 {
                    "NOTE: the project video is already in colour. Exemplar colorizes from scratch and only uses the brightness – \
                     better choose the original black-and-white video.".into()
                } else {
                    String::new()
                };
            }
        }
        self.kehys = k;
        if self.ehdotus_kehys != Some(f) {
            self.ehdotukset.clear();
            self.virheet.clear();
            self.ehdotus_kehys = None;
            if self.suositus.as_ref().map(|s| s.0 != f).unwrap_or(false) {
                self.suositus = None;
            }
            if self.kehys.is_some() && self.kp_tassa().is_some() && !self.tyo_kaynnissa {
                self.luo_ehdotukset();
            }
        }
    }

    fn lisaa_kiinnipiste(&mut self) {
        if self.e_tiedot.is_none() || self.kp_tassa().is_some() {
            return;
        }
        let ref_ = self.projekti.kiinnipisteet.iter().filter(|p| p.kehys < self.kohta).last().map(|p| p.referenssi.clone()).unwrap_or_default();
        self.projekti.kiinnipisteet.push(Kiinnipiste {
            kehys: self.kohta,
            referenssi: ref_,
            valinta: String::new(),
            resepti: Resepti::uusi(&self.s.e_ddcolor, "A: DDColor (automatic)"),
            f_pohja: "A".into(),
            analyysi: String::new(),
        });
        self.tallenna_projekti();
        self.hsv = (0.0, 1.0, 1.0);
        self.f_pohja = "A".into();
        if self.kehys.is_some() {
            self.ehdotus_kehys = None;
            self.luo_ehdotukset();
        }
    }

    fn poista_kiinnipiste(&mut self) {
        if let Some(i) = self.kp_tassa() {
            self.projekti.kiinnipisteet.remove(i);
            self.tallenna_projekti();
            self.ehdotukset.clear();
            self.ehdotus_kehys = None;
        }
    }

    fn hyppaa(&mut self, suunta: i32) {
        let k: Vec<usize> = self.projekti.kiinnipisteet.iter().map(|p| p.kehys).collect();
        if k.is_empty() {
            return;
        }
        let f = if suunta > 0 {
            k.iter().copied().find(|&f| f > self.kohta).unwrap_or(k[0])
        } else {
            k.iter().rev().copied().find(|&f| f < self.kohta).unwrap_or(*k.last().unwrap())
        };
        self.siirry(f);
    }

    fn automaattiset(&mut self) {
        let Some(t) = self.e_tiedot.clone() else { return };
        let (raja, n) = (self.s.leikkaus_raja, self.s.e_automaattisia.max(1));
        let seis = Arc::new(AtomicBool::new(false));
        self.taustatyo("Finding scenes…", move |tx| {
            let rajat = kohtausrajat(&t, raja, 0, 0, &seis)?;
            let kehykset = ehdota_kiinnipisteet(&rajat, t.n, n);
            let _ = tx.send(GuiViesti::Automaattiset(kehykset, rajat.len() + 1));
            Ok(())
        });
    }

    /// Kohtausanalyysi: jokaiseen kohtaukseen ehdotukset ja näkömallin valinta (taustalla, pysäytettävissä).
    fn aloita_analyysi(&mut self) {
        if self.analyysi_ajo.is_some() || self.e_tiedot.is_none() || self.s.e_projekti.is_empty() {
            return;
        }
        self.tallenna_projekti();
        let seis = Arc::new(AtomicBool::new(false));
        self.analyysi_ajo = Some(seis.clone());
        let a = crate::analyysi::AnalyysiAsetukset {
            leikkaus_raja: self.s.leikkaus_raja,
            ddcolor: self.s.e_ddcolor.clone(),
            laite: self.s.laite.clone(),
            nako_malli: self.s.nako_malli.clone(),
            min_kesto_s: self.s.analyysi_min_kesto,
            enintaan: self.s.analyysi_enintaan,
        };
        let (p, polku, tx) = (self.projekti.clone(), self.s.e_projekti.clone(), self.tx.clone());
        self.lokiin("\n===== Scene analysis =====");
        *self.tila.lock().unwrap() = ("Scene analysis running…".into(), 0.0);
        std::thread::spawn(move || {
            let kansio = crate::analyysi::kuvakansio(&polku);
            let tx2 = tx.clone();
            let tulos = crate::analyysi::kohtausanalyysi(&p, &kansio, &a, &seis, &mut |kp| {
                let _ = tx2.send(GuiViesti::AnalyysiKp(polku.clone(), kp));
            });
            let _ = tx.send(GuiViesti::AnalyysiValmis(tulos.map(|y| format!(
                "Scene analysis done: {} scenes analysed ({} scenes, {} short, {} already chosen, {} errors).",
                y.analysoitu, y.kohtauksia, y.lyhyita, y.valmiiksi_valittuja, y.virheita)).map_err(|e| format!("{e:#}"))));
        });
    }

    fn lisaa_automaattiset(&mut self, kehykset: Vec<usize>, kohtauksia: usize) {
        let fps = self.e_tiedot.as_ref().map(|t| t.fps).unwrap_or(25.0);
        let olemassa: Vec<usize> = self.projekti.kiinnipisteet.iter().map(|p| p.kehys).collect();
        let uudet: Vec<usize> = kehykset.into_iter().filter(|&f| olemassa.iter().all(|&o| (f as f64 - o as f64).abs() > fps)).collect();
        for &f in &uudet {
            self.projekti.kiinnipisteet.push(Kiinnipiste {
                kehys: f,
                resepti: Resepti::uusi(&self.s.e_ddcolor, "A: DDColor (automatic)"),
                f_pohja: "A".into(),
                ..Default::default()
            });
        }
        self.tallenna_projekti();
        self.lokiin(&format!("{kohtauksia} scenes, added {} keyframes. Go through them and pick the best version for each.", uudet.len()));
        if let Some(&f) = uudet.first() {
            self.siirry(f);
        }
    }

    fn luo_ehdotukset(&mut self) {
        let (Some(i), Some(k)) = (self.kp_tassa(), self.kehys.clone()) else { return };
        if self.tyo_kaynnissa {
            self.ehdotukset_jonossa = true;
            return;
        }
        self.ehdotukset_jonossa = false;
        let kp = &self.projekti.kiinnipisteet[i];
        let reseptit = ehdokkaat(&self.s.e_ddcolor, &kp.referenssi, self.kp_ref_voima);
        let (kehys, laite, refit, muisti) = (self.kohta, self.s.laite.clone(), self.refit.clone(), self.ab_muisti.clone());
        self.taustatyo("Creating suggestions (models load on the first run)…", move |tx| {
            for (j, r) in reseptit.into_iter().enumerate() {
                let tulos = match r {
                    None => (j, None, None, "Needs a reference image".to_string()),
                    Some(r) => {
                        let mut ab: HashMap<String, Kentta> =
                            muisti.lock().unwrap().iter().filter(|((f, _), _)| *f == kehys).map(|((_, m), v)| (m.clone(), v.clone())).collect();
                        match aja_resepti(&r, &k, &mut ab, &laite, &refit) {
                            Ok((l, a)) => {
                                let mut mm = muisti.lock().unwrap();
                                for (m, v) in ab {
                                    mm.insert((kehys, m), v);
                                }
                                (j, Some(r), Some(lab_bgr(&l, &a)), String::new())
                            }
                            Err(e) => (j, None, None, format!("{e}")),
                        }
                    }
                };
                let _ = tx.send(GuiViesti::Ehdotukset(kehys, vec![tulos], j == 4));
            }
            Ok(())
        });
    }

    fn paivita_f(&mut self, ctx: &egui::Context) {
        self.hsv_muuttui = false;
        let (Some(i), Some(k)) = (self.kp_tassa(), self.kehys.clone()) else { return };
        let Some(pohja) = self.ehdotukset.get(&self.f_pohja).map(|e| e.resepti.clone()) else { return };
        let mut r = pohja;
        (r.h, r.s, r.v) = self.hsv;
        r.nimi = format!("F: {} + HSV", self.f_pohja);
        let mut ab: HashMap<String, Kentta> =
            self.ab_muisti.lock().unwrap().iter().filter(|((f, _), _)| *f == self.kohta).map(|((_, m), v)| (m.clone(), v.clone())).collect();
        if r.mallit().iter().any(|m| !ab.contains_key(m)) {
            return; // pohjan malli ei vielä valmis
        }
        match aja_resepti(&r, &k, &mut ab, &self.s.laite, &self.refit) {
            Ok((l, a)) => {
                let kuva = lab_bgr(&l, &a);
                let tex = tekstuuri(ctx, "ehdotusF", &kuva);
                self.ehdotukset.insert("F".into(), Ehdotus { resepti: r.clone(), kuva: kuva.clone(), tex });
                if self.projekti.kiinnipisteet[i].valinta == "F" {
                    self.projekti.kiinnipisteet[i].resepti = r;
                    self.tallenna_kp_kuva(i, &kuva);
                    self.tallenna_projekti();
                    self.valitun_tex = None;
                }
            }
            Err(e) => {
                self.virheet.insert("F".into(), format!("{e}"));
            }
        }
    }

    fn valitse_ehdotus(&mut self, kirjain: &str) {
        let Some(i) = self.kp_tassa() else { return };
        let Some(e) = self.ehdotukset.get(kirjain) else { return };
        let (r, kuva) = (e.resepti.clone(), e.kuva.clone());
        let kehys = self.kohta;
        {
            let kp = &mut self.projekti.kiinnipisteet[i];
            if kp.valinta != kirjain {
                kp.analyysi.clear(); // käsin muutettu valinta ei ole enää analyysin tulos
            }
            kp.valinta = kirjain.to_string();
            kp.resepti = r.clone();
            if kirjain != "F" {
                kp.f_pohja = kirjain.to_string();
            }
        }
        if kirjain != "F" {
            self.f_pohja = kirjain.to_string();
            self.hsv_muuttui = true;
        }
        self.tallenna_kp_kuva(i, &kuva);
        self.tallenna_projekti();
        self.valitun_tex = None;
        if let Some(t) = &self.e_tiedot {
            crate::loki(&format!("Keyframe {}: chose {} – {}", aikateksti(t.aika(kehys)), r.nimi, r.kuvaus()));
        }
    }

    /// Pyytää näkömallilta suosituksen nykyisen kiinnipisteen ehdotuksista A–E (taustalla, ei jonota muita töitä).
    fn pyyda_suositus(&mut self) {
        if self.kp_tassa().is_none() || self.suositus_kaynnissa == Some(self.kohta) {
            return;
        }
        let ehdotukset: Vec<(String, String, Bgr)> = KIRJAIMET[..5]
            .iter()
            .filter_map(|k| self.ehdotukset.get(*k).map(|e| (k.to_string(), e.resepti.kuvaus(), e.kuva.clone())))
            .collect();
        if ehdotukset.len() < 2 {
            return;
        }
        let (kehys, malli, tx) = (self.kohta, self.s.nako_malli.clone(), self.tx.clone());
        self.suositus_kaynnissa = Some(kehys);
        self.suositus = None;
        std::thread::spawn(move || {
            let t = crate::nako::suosittele(&malli, &ehdotukset).map_err(|e| format!("{e:#}"));
            let _ = tx.send(GuiViesti::Suositus(kehys, t));
        });
    }

    fn tallenna_kp_kuva(&self, i: usize, k: &Bgr) {
        let p = self.kuvakansio().join(format!("kp_{:07}.jpg", self.projekti.kiinnipisteet[i].kehys));
        let _ = image::save_buffer(&p, &k.rgb(), k.w as u32, k.h as u32, image::ExtendedColorType::Rgb8);
    }

    fn valittu_kuva(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let i = self.kp_tassa()?;
        if let Some((f, t)) = &self.valitun_tex {
            if *f == self.kohta {
                return Some(t.clone());
            }
        }
        let kp = &self.projekti.kiinnipisteet[i];
        let kuva = if let (Some(e), Some(_)) = (self.ehdotukset.get(&kp.valinta), self.ehdotus_kehys.filter(|&f| f == self.kohta)) {
            Some(e.kuva.clone())
        } else {
            let p = self.kuvakansio().join(format!("kp_{:07}.jpg", kp.kehys));
            image::open(p).ok().map(|i| {
                let r = i.to_rgb8();
                Bgr::rgb_kuvasta(r.width() as usize, r.height() as usize, r.as_raw())
            })
        }?;
        let t = tekstuuri(ctx, "valittu", &kuva);
        self.valitun_tex = Some((self.kohta, t.clone()));
        Some(t)
    }

    fn ohje(&self) -> String {
        let kp = &self.projekti.kiinnipisteet;
        let mut t = if self.e_tiedot.is_none() {
            "Start: 'New project…' and choose a BLACK-AND-WHITE video.".to_string()
        } else if kp.is_empty() {
            "No keyframes. Move to an important spot on the timeline and press '+ Mark this frame' (5–15 of them), \
             or 'Automatic' adds one to each of the longest scenes.".to_string()
        } else {
            let ilman = kp.iter().filter(|p| p.valinta.is_empty()).count();
            if ilman > 0 {
                format!("{} keyframes, {ilman} without a chosen version – click the best suggestion A–F.", kp.len())
            } else {
                format!("{} keyframes, all have a chosen version. Ready to render.", kp.len())
            }
        };
        if !self.e_varoitus.is_empty() {
            t = format!("{}\n{t}", self.e_varoitus);
        }
        t
    }

    fn nayta_exemplar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if ui.button("New project…").clicked() {
                self.uusi_projekti();
            }
            if ui.button("Open project…").clicked() {
                if let Some(p) = pick("Open project", &[("Project", &["json"])], &self.s.e_projekti) {
                    self.avaa_projekti(&p);
                }
            }
            match &self.e_tiedot {
                Some(t) => ui.label(format!("{}   {}x{}  {:.3} fps  {}  ({} frames)", Path::new(&t.polku).file_name().unwrap_or_default().to_string_lossy(),
                                            t.w, t.h, t.fps, &aikateksti(t.kesto)[..7], t.n)),
                None => ui.label(egui::RichText::new("No project").color(Color32::GRAY)),
            };
        });
        ui.horizontal(|ui| {
            ui.label("Output:");
            let lev = ui.available_width() - 80.0;
            ui.add_sized([lev, 22.0], egui::TextEdit::singleline(&mut self.s.e_ulos));
            if ui.button("Browse…").clicked() {
                if let Some(p) = tallenna_nimella("Output", &[("MKV", &["mkv"]), ("MP4", &["mp4"])], &self.s.e_ulos) {
                    self.s.e_ulos = p;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Audio source (optional):");
            let lev = ui.available_width() - 80.0;
            ui.add_sized([lev, 22.0], egui::TextEdit::singleline(&mut self.s.e_aanilahde));
            if ui.button("Browse…").clicked() {
                if let Some(p) = pick("Audio source", &[("Video/Audio", VIDEOT)], &self.s.e_aanilahde) {
                    self.s.e_aanilahde = p;
                }
            }
        });
        ui.separator();
        let korkeus = ui.available_height();
        let oikea_lev = 3.0 * (PIKKU + 14.0) + 20.0;
        ui.horizontal_top(|ui| {
            // ---- vasen: esikatselu, aikajana, ohjaimet
            ui.vertical(|ui| {
                let lev = (ui.available_width() - oikea_lev - 12.0).max(300.0);
                let n_aika = self.e_tiedot.as_ref().map(|t| aikateksti(t.aika(self.kohta))).unwrap_or_default();
                let (tex, teksti) = if let Some(h) = self.hover.clone().and_then(|h| self.ehdotukset.get(&h).map(|e| (e.tex.clone(), e.resepti.nimi.clone()))) {
                    (Some(h.0), format!("{n_aika}  –  {}", h.1))
                } else if self.nayta_vari {
                    match self.valittu_kuva(ctx) {
                        Some(t) => {
                            let kuvaus = self.kp_tassa().map(|i| self.projekti.kiinnipisteet[i].resepti.kuvaus()).unwrap_or_default();
                            (Some(t), format!("{n_aika}  frame {}  –  {kuvaus}", self.kohta))
                        }
                        None => (self.kehys_tex.clone(), format!("{n_aika}  frame {}  (no chosen version)", self.kohta)),
                    }
                } else {
                    (self.kehys_tex.clone(), format!("{n_aika}  frame {}", self.kohta))
                };
                self.e_nakyma.piirra(ui, tex.as_ref(), &teksti, Vec2::new(lev, (korkeus - 110.0).max(200.0)));
                // aikajana
                let n = self.e_tiedot.as_ref().map(|t| t.n).unwrap_or(1).max(1);
                let (vastaus, maalari) = ui.allocate_painter(Vec2::new(lev, 40.0), Sense::click_and_drag());
                let r = vastaus.rect;
                maalari.rect_filled(r, 2.0, Color32::from_gray(40));
                let x = |f: usize| r.min.x + 8.0 + (r.width() - 16.0) * f as f32 / (n.max(2) - 1) as f32;
                maalari.line_segment([Pos2::new(r.min.x + 8.0, r.center().y + 4.0), Pos2::new(r.max.x - 8.0, r.center().y + 4.0)], Stroke::new(3.0, Color32::from_gray(120)));
                for p in &self.projekti.kiinnipisteet {
                    let px = x(p.kehys);
                    let vari = if p.kehys == self.kohta { Color32::from_rgb(255, 176, 0) } else if p.valinta.is_empty() { Color32::from_rgb(150, 150, 150) } else { Color32::from_rgb(79, 195, 247) };
                    maalari.add(egui::Shape::convex_polygon(vec![Pos2::new(px - 6.0, r.min.y + 4.0), Pos2::new(px + 6.0, r.min.y + 4.0), Pos2::new(px, r.min.y + 16.0)], vari, Stroke::NONE));
                }
                let px = x(self.kohta);
                maalari.line_segment([Pos2::new(px, r.min.y), Pos2::new(px, r.max.y)], Stroke::new(2.0, Color32::from_rgb(255, 64, 64)));
                if let Some(p) = vastaus.interact_pointer_pos() {
                    if vastaus.clicked() || vastaus.dragged() {
                        let f = (((p.x - r.min.x - 8.0) / (r.width() - 16.0)).clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
                        if f != self.kohta {
                            self.siirry(f);
                        }
                    }
                }
                // ohjaimet
                let fps = self.e_tiedot.as_ref().map(|t| t.fps.round() as i64).unwrap_or(25);
                ui.horizontal(|ui| {
                    for (t, d) in [("⏮", i64::MIN), ("⏪ 10 s", -10 * fps), ("◀ 1 s", -fps), ("◀ frame", -1), ("frame ▶", 1), ("1 s ▶", fps), ("10 s ⏩", 10 * fps), ("⏭", i64::MAX)] {
                        if ui.button(t).clicked() {
                            self.askel(d);
                        }
                    }
                    ui.add_sized([95.0, 22.0], egui::TextEdit::singleline(&mut self.e_aika));
                    if ui.button("Go").clicked() {
                        if let (Some(s), Some(t)) = (aika_sekunneiksi(&self.e_aika), &self.e_tiedot) {
                            let f = t.kehys(s);
                            self.siirry(f);
                        }
                    }
                    ui.label(format!("frame {} / {}", self.kohta, n - 1));
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.nayta_vari, false, "Show black-and-white");
                    ui.radio_value(&mut self.nayta_vari, true, "Show chosen version");
                    ui.label(egui::RichText::new("Arrows = frame, Shift+arrows = second, wheel = zoom").color(Color32::GRAY));
                });
            });
            // ---- oikea: kiinnipisteet ja ehdotukset (vieritettävä pienillä näytöillä)
            egui::ScrollArea::vertical().id_salt("oikea_palsta").max_height(korkeus).show(ui, |ui| ui.vertical(|ui| {
                ui.set_width(oikea_lev);
                ui.strong("Keyframes");
                let mut siirry = None;
                egui::ScrollArea::vertical().id_salt("kp_lista").max_height(120.0).show(ui, |ui| {
                    for p in &self.projekti.kiinnipisteet {
                        let aika = self.e_tiedot.as_ref().map(|t| aikateksti(t.aika(p.kehys))).unwrap_or_default();
                        let v = if p.valinta.is_empty() { "–" } else { p.valinta.as_str() };
                        // ◆ = näkömallin kohtausanalyysin valitsema, muut käsin
                        let merkki = if p.analyysi.is_empty() { " " } else { "◆" };
                        let teksti = egui::RichText::new(format!("{}{merkki}[{v}] {}", &aika[..aika.len().saturating_sub(2)], p.resepti.kuvaus())).monospace();
                        let v = ui.selectable_label(p.kehys == self.kohta, teksti);
                        let v = if p.analyysi.is_empty() { v } else { v.on_hover_text(&p.analyysi) };
                        if v.clicked() {
                            siirry = Some(p.kehys);
                        }
                    }
                });
                if let Some(f) = siirry {
                    self.siirry(f);
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("+ Mark this frame").clicked() {
                        self.lisaa_kiinnipiste();
                    }
                    if ui.add_enabled(!self.tyo_kaynnissa && self.e_tiedot.is_some(), egui::Button::new("Automatic")).clicked() {
                        self.automaattiset();
                    }
                    ui.add(egui::DragValue::new(&mut self.s.e_automaattisia).range(1..=60).suffix(" pcs"));
                    if self.analyysi_ajo.is_some() {
                        if ui.button("■ Stop analysis").clicked() {
                            if let Some(s) = &self.analyysi_ajo {
                                s.store(true, Ordering::Relaxed);
                            }
                        }
                    } else if ui.add_enabled(self.e_tiedot.is_some(), egui::Button::new("Scene analysis"))
                        .on_hover_text("The vision model goes through every scene, creates suggestions and picks the best colorization for each. \
                                        Hand-picked ones are not touched; an interrupted analysis resumes. Settings: Assistant tab.")
                        .clicked()
                    {
                        self.aloita_analyysi();
                    }
                    if ui.button("Remove").clicked() {
                        self.poista_kiinnipiste();
                    }
                    if ui.button("◀ Previous").clicked() {
                        self.hyppaa(-1);
                    }
                    if ui.button("Next ▶").clicked() {
                        self.hyppaa(1);
                    }
                });
                ui.label(egui::RichText::new(self.ohje()).color(Color32::from_rgb(200, 110, 30)));
                ui.separator();
                ui.strong("Versions of the selected keyframe");
                let kp_i = self.kp_tassa();
                ui.horizontal(|ui| {
                    ui.label("Reference:");
                    let mut ref_ = kp_i.map(|i| self.projekti.kiinnipisteet[i].referenssi.clone()).unwrap_or_default();
                    ui.add_sized([oikea_lev - 170.0, 22.0], egui::TextEdit::singleline(&mut ref_).interactive(false));
                    let mut uusi = None;
                    if ui.add_enabled(kp_i.is_some(), egui::Button::new("Browse…")).clicked() {
                        uusi = pick("Reference image for this keyframe", &[("Image", KUVAT)], &ref_);
                    }
                    if ui.add_enabled(kp_i.is_some(), egui::Button::new("×")).clicked() {
                        uusi = Some(String::new());
                    }
                    if let (Some(u), Some(i)) = (uusi, kp_i) {
                        self.projekti.kiinnipisteet[i].referenssi = u.clone();
                        self.tallenna_projekti();
                        if !u.is_empty() {
                            self.ehdotus_kehys = None;
                            self.luo_ehdotukset();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("DDColor:");
                    egui::ComboBox::from_id_salt("e_dd").width(160.0).selected_text(self.s.e_ddcolor.as_str()).show_ui(ui, |ui| {
                        for m in mallit::ddcolor_mallit() {
                            ui.selectable_value(&mut self.s.e_ddcolor, m.to_string(), m);
                        }
                    });
                    ui.add(egui::Slider::new(&mut self.kp_ref_voima, 0.0..=1.0).text("ref."));
                    if ui.add_enabled(kp_i.is_some() && !self.tyo_kaynnissa, egui::Button::new("Create suggestions")).clicked() {
                        self.ehdotus_kehys = None;
                        self.luo_ehdotukset();
                    }
                });
                // ehdotusruudukko
                self.hover = None;
                let valinta = kp_i.map(|i| self.projekti.kiinnipisteet[i].valinta.clone()).unwrap_or_default();
                let suositeltu = match &self.suositus {
                    Some((f, Ok(s))) if *f == self.kohta => Some(s.paras.clone()),
                    _ => None,
                };
                let mut valittu = None;
                egui::Grid::new("ehdotukset").num_columns(3).spacing([6.0, 6.0]).show(ui, |ui| {
                    for (n, k) in KIRJAIMET.iter().enumerate() {
                        let reunus = if *k == valinta {
                            Color32::from_rgb(255, 176, 0)
                        } else if suositeltu.as_deref() == Some(*k) {
                            Color32::from_rgb(70, 190, 90)
                        } else {
                            Color32::from_gray(70)
                        };
                        egui::Frame::new().stroke(Stroke::new(3.0, reunus)).inner_margin(2.0).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(PIKKU);
                                let koko = Vec2::new(PIKKU, PIKKU * 2.0 / 3.0);
                                let nimi = match self.ehdotukset.get(*k) {
                                    Some(e) => {
                                        let v = ui.add(egui::Image::new(&e.tex).fit_to_exact_size(koko).sense(Sense::click()));
                                        if v.hovered() {
                                            self.hover = Some(k.to_string());
                                        }
                                        if v.clicked() {
                                            valittu = Some(k.to_string());
                                        }
                                        e.resepti.nimi.clone()
                                    }
                                    None => {
                                        let (r, _) = ui.allocate_exact_size(koko, Sense::hover());
                                        ui.painter().rect_filled(r, 0.0, Color32::from_gray(45));
                                        let t = self.virheet.get(*k).cloned().unwrap_or_else(|| if self.tyo_kaynnissa && kp_i.is_some() { "computing…".into() } else { String::new() });
                                        format!("{k}: {t}")
                                    }
                                };
                                ui.add(egui::Label::new(egui::RichText::new(nimi).small()).wrap());
                            });
                        });
                        if n % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                if let Some(k) = valittu {
                    self.valitse_ehdotus(&k);
                }
                // näkömallin suositus
                if kp_i.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        let suositus = self.suositus.clone();
                        match (&suositus, self.suositus_kaynnissa) {
                            (_, Some(f)) if f == self.kohta => {
                                ui.spinner();
                                ui.label(format!("Vision model ({}) is rating the suggestions…", self.s.nako_malli));
                            }
                            (Some((f, Ok(s))), _) if *f == self.kohta => {
                                ui.label(egui::RichText::new(format!("Vision model recommends {} (runner-up {})", s.paras, s.toinen))
                                    .strong().color(Color32::from_rgb(70, 190, 90)));
                                let k = s.paras.clone();
                                if valinta != k && ui.button("Choose recommendation").clicked() {
                                    self.valitse_ehdotus(&k);
                                }
                                ui.label(egui::RichText::new(&s.perustelu).italics().small().color(Color32::GRAY));
                            }
                            (Some((f, Err(e))), _) if *f == self.kohta => {
                                ui.label(egui::RichText::new(format!("Vision model: {e}")).small().color(Color32::from_rgb(220, 90, 40)));
                            }
                            _ => {
                                if ui.add_enabled(self.ehdotukset.len() >= 2, egui::Button::new("Ask the vision model")).clicked() {
                                    self.pyyda_suositus();
                                }
                            }
                        }
                    });
                }
                ui.label(format!("F: manual adjustment, based on {}", self.f_pohja));
                let vanha = self.hsv;
                ui.add(egui::Slider::new(&mut self.hsv.0, -180.0..=180.0).text("Hue H°"));
                ui.add(egui::Slider::new(&mut self.hsv.1, 0.0..=3.0).text("Saturation S"));
                ui.add(egui::Slider::new(&mut self.hsv.2, 0.6..=1.4).text("Brightness V"));
                if ui.button("Reset HSV").clicked() {
                    self.hsv = (0.0, 1.0, 1.0);
                }
                if self.hsv != vanha {
                    self.hsv_muuttui = true;
                }
            }));
        });
        if self.hsv_muuttui && !ctx.input(|i| i.pointer.any_down()) {
            self.paivita_f(ctx);
        }
        // näppäimet
        if !ctx.egui_wants_keyboard_input() {
            let (vasen, oikea, shift) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.modifiers.shift));
            let fps = self.e_tiedot.as_ref().map(|t| t.fps.round() as i64).unwrap_or(25);
            let d = if shift { fps } else { 1 };
            if vasen {
                self.askel(-d);
            }
            if oikea {
                self.askel(d);
            }
        }
    }

    fn askel(&mut self, d: i64) {
        let Some(t) = &self.e_tiedot else { return };
        let n = t.n as i64;
        let f = match d {
            i64::MIN => 0,
            i64::MAX => n - 1,
            _ => (self.kohta as i64 + d).clamp(0, n - 1),
        };
        self.siirry(f as usize);
    }
}

impl eframe::App for Sovellus {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.kasittele_viestit(ctx);
        if self.tyo_kaynnissa || self.ajo.is_some() || self.lisaosa_ajo.is_some() || self.ollama_haku || self.haku_kaynnissa.load(Ordering::Relaxed)
            || self.laatu_ajo.is_some() || self.suositus_kaynnissa.is_some() || self.analyysi_ajo.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        #[cfg(feature = "itsetesti")]
        if let Some(mut t) = self.testi.take() {
            if t.askel(self) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.testi = Some(t);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Some(s) = &self.ajo {
                s.store(true, Ordering::Relaxed);
            }
            if let Some(s) = &self.laatu_ajo {
                s.store(true, Ordering::Relaxed);
            }
            if let Some(s) = &self.analyysi_ajo {
                s.store(true, Ordering::Relaxed);
            }
            self.tallenna_asetukset();
            self.tallenna_projekti();
            crate::nako::sammuta_oma_palvelin();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("valilehdet").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (v, n) in [(Valilehti::Varita, "1. Colorize"), (Valilehti::Korjaa, "2. Repair"), (Valilehti::Exemplar, "3. Exemplar (keyframes)"),
                               (Valilehti::Asetukset, "Stabilization & output"), (Valilehti::Lisaosat, "Add-ons"),
                               (Valilehti::Avustaja, "Assistant")] {
                    if ui.selectable_label(self.valilehti == v, egui::RichText::new(n).size(15.0)).clicked() {
                        self.valilehti = v;
                        if matches!(v, Valilehti::Varita | Valilehti::Korjaa | Valilehti::Exemplar) {
                            self.tila_valilehti = v;
                        }
                    }
                }
            });
        });
        egui::Panel::bottom("ala").resizable(true).default_size(170.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let teksti = match self.tila_valilehti {
                    Valilehti::Korjaa => "▶ Start repair",
                    Valilehti::Exemplar => "▶ Render final video",
                    _ => "▶ Start colorizing",
                };
                if ui.add_enabled(self.ajo.is_none(), egui::Button::new(egui::RichText::new(teksti).strong())).clicked() {
                    self.aloita();
                }
                if ui.add_enabled(self.ajo.is_some(), egui::Button::new("■ Stop")).clicked() {
                    self.pysayta();
                }
                ui.separator();
                if ui.add_enabled(self.ajo.is_none(), egui::Button::new("🎲 Random preview"))
                    .on_hover_text("Runs a short clip from a random spot with the current mode's settings and plays it")
                    .clicked()
                {
                    self.aloita_esikatselu();
                }
                ui.add(egui::DragValue::new(&mut self.s.esik_kesto).range(1.0..=120.0).speed(0.5).suffix(" s"));
                ui.separator();
                if ui.button("Save settings").clicked() {
                    self.tallenna_asetukset();
                    crate::loki(&format!("Settings saved: {}", asetustiedosto().display()));
                }
                if ui.button("Clear log").clicked() {
                    self.loki.lock().unwrap().clear();
                }
                if self.s.nayte_kesto > 0.0 || self.s.nayte_alku > 0.0 {
                    ui.separator();
                    ui.label(egui::RichText::new(format!("⚠ Sample: only {} from {}",
                        if self.s.nayte_kesto > 0.0 { format!("{:.0} s", self.s.nayte_kesto) } else { "to the end".into() },
                        crate::video::aikateksti(self.s.nayte_alku))).color(Color32::from_rgb(220, 90, 40)).strong());
                    if ui.button("Whole video").clicked() {
                        self.s.nayte_alku = 0.0;
                        self.s.nayte_kesto = 0.0;
                    }
                }
            });
            let (t, p) = self.tila.lock().unwrap().clone();
            ui.add(egui::ProgressBar::new(p).desired_height(14.0));
            ui.label(t);
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for rivi in self.loki.lock().unwrap().iter() {
                    ui.label(egui::RichText::new(rivi).monospace().size(12.0));
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| match self.valilehti {
            Valilehti::Varita => self.nayta_varita(ui),
            Valilehti::Korjaa => self.nayta_korjaa(ui),
            Valilehti::Exemplar => self.nayta_exemplar(ui, &ctx),
            Valilehti::Asetukset => {
                egui::ScrollArea::vertical().show(ui, |ui| self.nayta_asetukset(ui));
            }
            Valilehti::Lisaosat => {
                egui::ScrollArea::vertical().show(ui, |ui| self.nayta_lisaosat(ui));
            }
            Valilehti::Avustaja => {
                egui::ScrollArea::vertical().show(ui, |ui| self.nayta_avustaja(ui));
            }
        });
        self.nayta_soitin(&ctx);
    }
}
