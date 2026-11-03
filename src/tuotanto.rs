//! Videon käsittely kolmessa tilassa:
//!   vari      mustavalko -> väri yhdellä reseptillä
//!   korjaa    jo väritetyn videon vakautus (+ harmaiden ruutujen ja läikkien täyttö)
//!   exemplar  kiinnipisteiden reseptit interpoloidaan pehmeästi (S-käyrä, LCh)
//! Kaikissa tiloissa lopuksi optinen virtaus + aikatasaus ja värien lukitus.
//! Säikeet: lukija -> mallit (tämä säie) -> vakautus + kirjoitus.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::kuva::{Bgr, Kentta, Taso, alueet, laajenna};
use crate::resepti::{Projekti, Referenssit, Resepti, sovella};
use crate::vakautus::{Aikatasaus, Leikkaukset, Varilukitus, Varimuisti, pieni_harmaa};
use crate::vari::{bgr_lab, l_kanava, lab_bgr, lch_sekoitus, s_kayra};
use crate::video::{Kirjoittaja, Kirjoitusasetukset, Lukija, Tiedot, aikateksti};
use crate::putki::Laadunvalvonta;
use crate::{edistys, loki, mallit};

const ERA: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Asetukset {
    pub tila: String,
    pub video: String,
    pub ulos: String,
    pub alkuperainen: String,
    pub projekti: String,
    pub resepti: Resepti,
    pub laite: String,
    pub tyoleveys: usize,
    pub ikkuna: usize,
    pub virtaus: bool,
    pub tasaus_voima: f32,
    /// Värimuisti: kuinka paljon edellisistä kuvista liikkeen mukana kuljetettu väri painaa (0 = pois)
    pub muisti: f32,
    /// Kohtausmuisti (DINOv2): sama kohde samanvärinen leikkausten yli
    pub kohtausmuisti: bool,
    pub lukitus: f32,
    pub lukitus_luokat: Vec<String>,
    pub leikkaus_raja: f32,
    pub alku_s: f64,
    pub kesto_s: f64,
    pub koodekki: String,
    pub crf: u32,
    pub tayta_harmaat: bool,
    pub tayta_malli: String,
    pub harmaa_raja: f32,
    /// Äänilähde: tyhjä = videosta, muuten tiedostopolku ääniraitaan
    pub aanilahde: String,
    /// Kuvakohtainen laadunvalvonta (läikät, harmaat ruudut, vertailu edellisiin kuviin)
    pub valvonta: bool,
    /// Korjauksessa: mistä kohtaa (s) alkuperäistä luetaan, kun korjattava on näytepätkä
    pub alkup_siirto_s: f64,
    /// Värityksessä: säännölliset kopioruudut pudotetaan (tulos todellisella kuvanopeudella, esim. 30 -> 25)
    /// ja satunnaisille kopioille käytetään edellisen kuvan väriä
    pub poista_kopiot: bool,
}

impl Default for Asetukset {
    fn default() -> Self {
        Asetukset {
            tila: "vari".into(),
            video: String::new(),
            ulos: String::new(),
            alkuperainen: String::new(),
            projekti: String::new(),
            resepti: Resepti::default(),
            laite: "auto".into(),
            tyoleveys: 960,
            ikkuna: 7,
            virtaus: true,
            tasaus_voima: 1.0,
            muisti: 0.98,
            kohtausmuisti: true,
            lukitus: 0.6,
            lukitus_luokat: vec!["iho".into(), "taivas".into(), "kasvit".into()],
            leikkaus_raja: 0.35,
            alku_s: 0.0,
            kesto_s: 0.0,
            koodekki: "x264".into(),
            crf: 14,
            tayta_harmaat: true,
            tayta_malli: "ddcolor_suomi_technicolor".into(),
            harmaa_raja: 4.0,
            aanilahde: String::new(),
            valvonta: false,
            alkup_siirto_s: 0.0,
            poista_kopiot: true,
        }
    }
}

/// Ajaa tilan mukaisen työn: "putki" (väritys → tarkistus → korjaus) tai yksittäinen vaihe.
pub fn suorita(a: Asetukset, seis: Arc<AtomicBool>) -> Result<String> {
    if a.tila == "putki" {
        crate::putki::aja(a, seis)
    } else {
        Tuottaja::new(a, seis)?.aja()
    }
}

pub struct Edistys {
    nimi: String,
    n: usize,
    t0: Instant,
    viim: Instant,
}

impl Edistys {
    pub fn new(nimi: &str, n: usize) -> Self {
        let t = Instant::now();
        Edistys { nimi: nimi.into(), n: n.max(1), t0: t, viim: t - std::time::Duration::from_secs(1) }
    }
    pub fn paivita(&mut self, i: usize) {
        if self.viim.elapsed().as_secs_f32() < 0.4 {
            return;
        }
        self.viim = Instant::now();
        let nopeus = i as f64 / self.t0.elapsed().as_secs_f64().max(1e-6);
        let jaljella = (self.n.saturating_sub(i)) as f64 / nopeus.max(1e-6);
        edistys(&format!("{}: {i}/{} frames ({:.0} %)  {nopeus:.1} frames/s  remaining ~{}", self.nimi, self.n,
                         100.0 * i as f64 / self.n as f64, &aikateksti(jaljella)[..7]), i, self.n);
    }
    pub fn valmis(&mut self) {
        loki(&format!("{}: done, {} frames, time {}", self.nimi, self.n, &aikateksti(self.t0.elapsed().as_secs_f64())[..7]));
    }
}

type Tyo = (Taso, Taso, Kentta, f32, bool); // L täysi, L työkoko, ab työkoko, v, leikkaus

struct Jalkikasittely {
    tx: Option<SyncSender<Tyo>>,
    saie: Option<JoinHandle<Result<usize>>>,
}

impl Jalkikasittely {
    fn kaynnista(a: &Asetukset, t: &Tiedot, mut kirj: Kirjoittaja, seis: Arc<AtomicBool>) -> Jalkikasittely {
        let (tx, rx): (SyncSender<Tyo>, Receiver<Tyo>) = sync_channel(24);
        let (ikkuna, virtaus, voima, lukitus, luokat) = (a.ikkuna, a.virtaus, a.tasaus_voima, a.lukitus, a.lukitus_luokat.clone());
        let muisti_voima = a.muisti;
        let mut kohtausmuisti = if a.kohtausmuisti && a.muisti > 0.0 {
            if crate::piirteet::on_ladattu() {
                match crate::kohtausmuisti::Kohtausmuisti::new() {
                    Ok(k) => Some(k),
                    Err(e) => {
                        loki(&format!("Scene memory not in use: {e:#}"));
                        None
                    }
                }
            } else {
                loki("Scene memory not in use: the DINOv2 model is not installed (Add-ons).");
                None
            }
        } else {
            None
        };
        let (w, h) = (t.w, t.h);
        let saie = std::thread::spawn(move || -> Result<usize> {
            let mut tasaus: Aikatasaus<(Taso, Taso, f32, bool)> = Aikatasaus::new(ikkuna, virtaus, voima);
            let mut lukko = Varilukitus::new(lukitus, &luokat);
            let mut muisti = Varimuisti::new(muisti_voima);
            let mut ensimmainen = true;
            let mut n = 0usize;
            let mut kirjoita = |d: (Taso, Taso, f32, bool), ab: Kentta, kirj: &mut Kirjoittaja| -> Result<()> {
                let (l_full, lw, v, leik) = d;
                let ab = lukko.kasittele(&lw, ab, leik).koon(w, h);
                let l = if v != 1.0 {
                    let mut l = l_full;
                    l.data.iter_mut().for_each(|x| *x = (*x * v).clamp(0.0, 100.0));
                    l
                } else {
                    l_full
                };
                kirj.kirjoita(&lab_bgr(&l, &ab))?;
                n += 1;
                Ok(())
            };
            let tulos = (|| -> Result<()> {
                for (l_full, lw, ab, v, leik) in rx.iter() {
                    if seis.load(Ordering::Relaxed) {
                        bail!("Cancelled.");
                    }
                    let g = pieni_harmaa(&lw);
                    let alku = leik || ensimmainen;
                    ensimmainen = false;
                    if leik {
                        muisti.nollaa();
                    }
                    if let Some(km) = kohtausmuisti.as_mut().filter(|_| alku) {
                        match km.kohtauksen_alku(&g) {
                            Ok(Some((s, p))) => muisti.aseta_siemen(s, p),
                            Ok(None) => {}
                            Err(e) => loki(&format!("Scene memory: {e:#}")),
                        }
                    }
                    let v_ed = if muisti.kaytossa() { muisti.virtaus_edelliseen(&g, leik) } else { None };
                    let ab = muisti.kasittele(&g, ab, v_ed.as_ref());
                    if let Some(km) = kohtausmuisti.as_mut() {
                        if let Err(e) = km.paivita(&g, muisti.muisti_ab()) {
                            loki(&format!("Scene memory: {e:#}"));
                        }
                    }
                    for (d, ab) in tasaus.lisaa((l_full, lw, v, leik), ab, g, leik, v_ed) {
                        kirjoita(d, ab, &mut kirj)?;
                    }
                }
                if seis.load(Ordering::Relaxed) {
                    bail!("Cancelled.");
                }
                for (d, ab) in tasaus.tyhjenna() {
                    kirjoita(d, ab, &mut kirj)?;
                }
                Ok(())
            })();
            if let Some(km) = &kohtausmuisti {
                loki(&format!("Scene memory: {}.", km.yhteenveto()));
            }
            match tulos {
                Ok(()) => {
                    kirj.sulje()?;
                    Ok(n)
                }
                Err(e) => {
                    kirj.keskeyta();
                    Err(e)
                }
            }
        });
        Jalkikasittely { tx: Some(tx), saie: Some(saie) }
    }

    fn syota(&mut self, tyo: Tyo) -> Result<()> {
        if self.tx.as_ref().unwrap().send(tyo).is_err() {
            // jälkikäsittely on pysähtynyt virheeseen: haetaan virhe
            return Err(self.lopeta().err().unwrap_or_else(|| anyhow!("post-processing stopped")));
        }
        Ok(())
    }

    fn lopeta(&mut self) -> Result<usize> {
        drop(self.tx.take());
        match self.saie.take() {
            Some(s) => s.join().map_err(|_| anyhow!("post-processing crashed"))?,
            None => Err(anyhow!("post-processing already finished")),
        }
    }
}

/// Lukee kehyksiä omassa säikeessään.
fn lukijasaie(t: &Tiedot, alku: usize, maara: usize, koko: Option<(usize, usize)>) -> Result<Receiver<Bgr>> {
    let mut l = Lukija::avaa(t, alku, maara, koko)?;
    let (tx, rx) = sync_channel(16);
    std::thread::spawn(move || {
        while let Some(k) = l.seuraava() {
            if tx.send(k).is_err() {
                break;
            }
        }
    });
    Ok(rx)
}

pub struct Tuottaja {
    pub a: Asetukset,
    t: Tiedot,
    alku: usize,
    maara: usize,
    ww: usize,
    wh: usize,
    refit: Referenssit,
    seis: Arc<AtomicBool>,
}

impl Tuottaja {
    pub fn new(mut a: Asetukset, seis: Arc<AtomicBool>) -> Result<Tuottaja> {
        if a.tila == "exemplar" && a.video.is_empty() {
            a.video = Projekti::lue(&a.projekti)?.video;
        }
        let mut t = Tiedot::lue(&a.video)?;
        if a.tila == "vari" && a.poista_kopiot {
            if let Some(n) = t.tunnista_kadenssi()? {
                let lahde = t.fps;
                t.harvenna(n);
                loki(&format!("Duplicate frames: every {n}th frame is a copy of the previous one – the {lahde:.3} fps source is really \
                               {:.3} fps. Copies are dropped and the result is written at {} fps (duration and sound unchanged).",
                              t.fps, t.fps_teksti()));
            }
        }
        let alku = if a.alku_s > 0.0 { t.kehys(a.alku_s) } else { 0 };
        let loput = t.n.saturating_sub(alku);
        let maara = if a.kesto_s > 0.0 { t.kehys(a.kesto_s).min(loput) } else { loput };
        let tw = t.w.min(a.tyoleveys.max(64)) / 2 * 2;
        let th = (((t.h as f64 * tw as f64 / t.w as f64).round() as usize) / 2 * 2).max(2);
        if maara == 0 {
            bail!("Start position {} is after the end of the video ({}) – nothing to process.", aikateksti(a.alku_s), aikateksti(t.kesto));
        }
        if a.ulos.is_empty() {
            let p = Path::new(&a.video);
            let paate = match a.tila.as_str() { "korjaa" => "_repaired", "vari" | "putki" => "_colorized", _ => "_exemplar" };
            a.ulos = p.with_file_name(format!("{}{paate}.mkv", p.file_stem().unwrap().to_string_lossy())).to_string_lossy().into();
        }
        if Path::new(&a.ulos).canonicalize().ok() == Path::new(&a.video).canonicalize().ok() && Path::new(&a.ulos).exists() {
            bail!("The output file cannot be the same as the source video.");
        }
        loki(&format!("Video: {}x{} @ {:.3} fps, {} frames ({}), processing {maara} frames from {}",
                      t.w, t.h, t.fps, t.n, &aikateksti(t.kesto)[..7], aikateksti(t.aika(alku))));
        loki(&format!("Colour processing size {tw}x{th}, colour memory {:.2}, temporal smoothing {} frames {}, lock {:.2}", a.muisti, a.ikkuna,
                      if a.virtaus { "with optical flow" } else { "without optical flow" }, a.lukitus));
        Ok(Tuottaja { a, t, alku, maara, ww: tw, wh: th, refit: Referenssit::default(), seis })
    }

    pub fn alku_s(&self) -> f64 {
        if self.alku > 0 { self.t.aika(self.alku) } else { 0.0 }
    }

    pub fn kesto_s(&self) -> f64 {
        self.t.aika(self.maara)
    }

    fn kirjoitusasetukset<'a>(&'a self, aanilahde_override: Option<&'a str>) -> Kirjoitusasetukset<'a> {
        let aanilahde = aanilahde_override.or_else(|| {
            if self.a.aanilahde.is_empty() {
                None
            } else {
                Some(self.a.aanilahde.as_str())
            }
        });
        // Ulkopuolinen ääni = käyttäjä on antanut eksplisiittisesti aanilahde-asetuksen
        let aanilahde_ulkopuolinen = aanilahde_override.is_some() || (!self.a.aanilahde.is_empty() && self.a.aanilahde != self.a.video);
        Kirjoitusasetukset {
            aanilahde,
            aanilahde_ulkopuolinen,
            alku_s: if self.alku > 0 { self.t.aika(self.alku) } else { 0.0 },
            kesto_s: if self.a.kesto_s > 0.0 { self.t.aika(self.maara) } else { 0.0 },
            koodekki: &self.a.koodekki,
            crf: self.a.crf,
        }
    }

    fn tarkista_seis(&self) -> Result<()> {
        if self.seis.load(Ordering::Relaxed) { Err(anyhow!("Cancelled.")) } else { Ok(()) }
    }

    pub fn aja(&mut self) -> Result<String> {
        self.tarkista_aikajana_ennen();
        let n = match self.a.tila.as_str() {
            "vari" => self.vari()?,
            "korjaa" => self.korjaa()?,
            "exemplar" => self.exemplar()?,
            t => bail!("unknown mode {t}"),
        };
        loki(&format!("Output: {}  ({n} frames)", self.a.ulos));
        self.tarkista_aikajana_jalkeen();
        Ok(self.a.ulos.clone())
    }

    /// Kuvanopeus ja äänilähde ennen ajoa.
    fn tarkista_aikajana_ennen(&self) {
        let t = &self.t;
        if t.vfr {
            loki(&format!("The source has a variable frame rate (average {:.3} fps): processing at a fixed {} fps \
                           so that the duration and sound of the result match the source.", t.keski_fps, t.fps_teksti()));
        }
        if !self.a.aanilahde.is_empty() {
            match Tiedot::lue(&self.a.aanilahde) {
                Ok(aa) if !aa.aanta => loki(&format!("WARNING: the audio source {} has no audio track.", self.a.aanilahde)),
                Ok(aa) => {
                    let ero = aa.kesto - t.kesto;
                    if ero.abs() > (0.005 * t.kesto).max(1.0) {
                        loki(&format!(
                            "WARNING: the audio source duration {} differs from the video duration {} ({:+.1} s, {:+.1} %). Sound will drift: \
                             the versions run at different speeds or are different cuts. Better colorize the version that has the sound.",
                            &aikateksti(aa.kesto)[..7], &aikateksti(t.kesto)[..7], ero, 100.0 * ero / t.kesto));
                    }
                }
                Err(e) => loki(&format!("WARNING: cannot read the audio source: {e}")),
            }
        }
    }

    /// Varmistaa, että tuloksen kuvanopeus ja kesto vastaavat lähdettä (ääni pysyy tahdissa).
    fn tarkista_aikajana_jalkeen(&self) {
        let Ok(u) = Tiedot::lue(&self.a.ulos) else { return };
        let odotettu = self.maara as f64 / self.t.fps;
        let kesto = u.n as f64 / u.fps;
        let sama_fps = (u.fps - self.t.fps).abs() < 0.001;
        let sama_kesto = (kesto - odotettu).abs() <= 2.0 / self.t.fps;
        if sama_fps && sama_kesto {
            loki(&format!("Timeline OK: {} fps, {} frames, duration {} = source.", u.fps_teksti(), u.n, aikateksti(kesto)));
        } else {
            loki(&format!("WARNING: the result's timeline differs from the source: {:.3} fps / duration {} (source {:.3} fps / {}).",
                          u.fps, aikateksti(kesto), self.t.fps, aikateksti(odotettu)));
        }
    }

    fn mallien_ab(&self, nimet: &[String], kuvat: &[&Bgr]) -> Result<HashMap<String, Vec<Kentta>>> {
        let mut m = HashMap::new();
        for nimi in nimet {
            let malli = mallit::hae(nimi, &self.a.laite)?;
            let ab = malli.lock().unwrap().ab(kuvat)?;
            m.insert(nimi.clone(), ab);
        }
        Ok(m)
    }

    // ---------------------------------------------------------------- väritä
    fn vari(&mut self) -> Result<usize> {
        let r = self.a.resepti.clone();
        loki(&format!("Colorizing: {}", r.kuvaus()));
        if let Some(v) = r.saatovaroitus() {
            loki(&format!("WARNING: {v}. For natural colours: H 0, S 1.0."));
        }
        let nimet: Vec<String> = r.mallit().into_iter().collect();
        for nimi in &nimet {
            mallit::hae(nimi, &self.a.laite)?;
        }
        let kirj = Kirjoittaja::avaa(&self.a.ulos, &self.t, &self.kirjoitusasetukset(None))?;
        let mut jk = Jalkikasittely::kaynnista(&self.a, &self.t, kirj, self.seis.clone());
        let rx = lukijasaie(&self.t, self.alku, self.maara, None)?;
        let mut leik = Leikkaukset::new(self.a.leikkaus_raja);
        let mut valvonta = if self.a.valvonta {
            Some(Laadunvalvonta::new(&self.a.tayta_malli, Some(&r.menetelma), &self.a.laite, self.a.harmaa_raja)?)
        } else {
            None
        };
        let mut ed = Edistys::new("Colorizing", self.maara);
        let mut i = 0;
        // satunnaiset kopioruudut: viimeisin mallilla väritetty kuva (pienoiskuva ja väri)
        let mut viite: Option<(Vec<u8>, Kentta)> = None;
        let mut kopioita = 0usize;
        let tulos = (|| -> Result<()> {
            loop {
                self.tarkista_seis()?;
                let era: Vec<Bgr> = rx.iter().take(ERA).collect();
                if era.is_empty() {
                    return Ok(());
                }
                let pienet: Vec<Bgr> = era.iter().map(|k| k.koon(self.ww, self.wh)).collect();
                let lt: Vec<(Taso, Taso)> = era.iter().map(|k| {
                    let l_full = l_kanava(k);
                    let lw = l_full.koon(self.ww, self.wh);
                    (l_full, lw)
                }).collect();
                let leikkaukset: Vec<bool> = lt.iter().map(|(_, lw)| leik.onko(&pieni_harmaa(lw))).collect();
                // kopio = lähes sama kuin viimeisin mallilla väritetty (ei edellinen, ettei hidas muutos kerry)
                let pienoiset: Vec<Vec<u8>> = if self.a.poista_kopiot { pienet.iter().map(kopiotunniste).collect() } else { vec![] };
                let mut kopio = vec![false; era.len()];
                if self.a.poista_kopiot {
                    let mut ref_p = viite.as_ref().map(|v| v.0.clone());
                    for j in 0..era.len() {
                        kopio[j] = !leikkaukset[j] && ref_p.as_ref().map(|p| on_kopio(p, &pienoiset[j])).unwrap_or(false);
                        if !kopio[j] {
                            ref_p = Some(pienoiset[j].clone());
                        }
                    }
                }
                let lasketaan: Vec<usize> = (0..era.len()).filter(|&j| !kopio[j]).collect();
                let viitteet: Vec<&Bgr> = lasketaan.iter().map(|&j| &pienet[j]).collect();
                let mut ab = if viitteet.is_empty() { HashMap::new() } else { self.mallien_ab(&nimet, &viitteet)? };
                let mut m = 0;
                for (j, (l_full, lw)) in lt.into_iter().enumerate() {
                    let c = leikkaukset[j];
                    let abr = if kopio[j] {
                        kopioita += 1;
                        viite.as_ref().unwrap().1.clone()
                    } else {
                        let mab: HashMap<String, Kentta> = ab.iter_mut().map(|(n, v)| (n.clone(), std::mem::replace(&mut v[m], Kentta::new(1, 1)))).collect();
                        m += 1;
                        let (_, abr) = sovella(&r, &lw, &mab, &self.refit)?;
                        let abr = match valvonta.as_mut() {
                            Some(v) => v.kasittele(&pienet[j], &lw, abr, c)?,
                            None => abr,
                        };
                        if self.a.poista_kopiot {
                            viite = Some((pienoiset[j].clone(), abr.clone()));
                        }
                        abr
                    };
                    jk.syota((l_full, lw, abr, r.v, c))?;
                }
                i += era.len();
                ed.paivita(i);
            }
        })();
        if let Err(e) = tulos {
            self.seis.store(true, Ordering::Relaxed);
            let _ = jk.lopeta();
            self.seis.store(false, Ordering::Relaxed);
            return Err(e);
        }
        let n = jk.lopeta()?;
        ed.valmis();
        if kopioita > 0 {
            loki(&format!("Duplicate frames: {kopioita} frames reused the previous frame's colour (model run skipped)."));
        }
        if let Some(v) = &valvonta {
            loki(&format!("Quality control: {}.", v.yhteenveto()));
        }
        Ok(n)
    }

    // ---------------------------------------------------------------- korjaa
    fn korjaa(&mut self) -> Result<usize> {
        let mut alkup_rx = None;
        let mut aani = self.a.video.clone();
        if !self.a.alkuperainen.is_empty() {
            let mut ta = Tiedot::lue(&self.a.alkuperainen)?;
            if (ta.fps - self.t.fps).abs() > 0.01 {
                // väritetty tulos on voitu kirjoittaa todellisella nopeudella (kopioruudut pudotettu)
                if let Some(n) = ta.tunnista_kadenssi()? {
                    let mut h = ta.clone();
                    h.harvenna(n);
                    if (h.fps - self.t.fps).abs() <= 0.01 {
                        loki(&format!("Duplicate frames (every {n}th) are dropped from the original as in colorizing."));
                        ta = h;
                    }
                }
            }
            if (ta.fps - self.t.fps).abs() > 0.01 {
                loki(&format!("WARNING: frame rates differ ({:.3} vs {:.3}); brightness is taken from the colorized video.", ta.fps, self.t.fps));
            } else {
                // pätkää (alkup_siirto_s) verrataan koko alkuperäiseen, joten kuvamäärät eroavat luonnostaan
                if self.a.alkup_siirto_s == 0.0 && (ta.n as i64 - self.t.n as i64).abs() > 2 {
                    loki(&format!("WARNING: frame counts differ ({} vs {}), check that the videos are the same.", ta.n, self.t.n));
                }
                let siirto = if self.a.alkup_siirto_s > 0.0 { ta.kehys(self.a.alkup_siirto_s) } else { 0 };
                alkup_rx = Some(lukijasaie(&ta, self.alku + siirto, self.maara, Some((self.t.w, self.t.h)))?);
                if !self.t.aanta {
                    aani = self.a.alkuperainen.clone();
                }
                loki("Brightness is taken from the original black-and-white video.");
            }
        }
        let mut valvonta = if self.a.tayta_harmaat {
            Some(Laadunvalvonta::new(&self.a.tayta_malli, None, &self.a.laite, self.a.harmaa_raja)?)
        } else {
            None
        };
        // Äänilähde: jos asetuksissa on annettu, sitä käytetään; muuten aani-muuttujan logiikka
        let aanilahde = if self.a.aanilahde.is_empty() { Some(aani.as_str()) } else { None };
        let kirj = Kirjoittaja::avaa(&self.a.ulos, &self.t, &self.kirjoitusasetukset(aanilahde))?;
        let mut jk = Jalkikasittely::kaynnista(&self.a, &self.t, kirj, self.seis.clone());
        let rx = lukijasaie(&self.t, self.alku, self.maara, None)?;
        let mut leik = Leikkaukset::new(self.a.leikkaus_raja);
        let mut ed = Edistys::new("Repair", self.maara);
        let tulos = (|| -> Result<()> {
            for (i, k) in rx.iter().enumerate() {
                self.tarkista_seis()?;
                let kw = k.koon(self.ww, self.wh);
                let lab_w = bgr_lab(&kw);
                let o = match &alkup_rx {
                    Some(r) => r.recv().unwrap_or_else(|_| k.clone()),
                    None => k.clone(),
                };
                let l_full = l_kanava(&o);
                let lw = if alkup_rx.is_some() { l_full.koon(self.ww, self.wh) } else { lab_w.l };
                let c = leik.onko(&pieni_harmaa(&lw));
                let ab = match valvonta.as_mut() {
                    Some(v) => v.kasittele(&o.koon(self.ww, self.wh), &lw, lab_w.ab, c)?,
                    None => lab_w.ab,
                };
                jk.syota((l_full, lw, ab, 1.0, c))?;
                ed.paivita(i + 1);
            }
            Ok(())
        })();
        if let Err(e) = tulos {
            self.seis.store(true, Ordering::Relaxed);
            let _ = jk.lopeta();
            self.seis.store(false, Ordering::Relaxed);
            return Err(e);
        }
        let n = jk.lopeta()?;
        ed.valmis();
        if let Some(v) = &valvonta {
            loki(&format!("Quality control: {}.", v.yhteenveto()));
        }
        Ok(n)
    }

    // ---------------------------------------------------------------- exemplar
    fn exemplar(&mut self) -> Result<usize> {
        let projekti = Projekti::lue(&self.a.projekti)?;
        let mut pisteet: Vec<(usize, Resepti)> = projekti.kiinnipisteet.iter().map(|p| (p.kehys, p.resepti.clone())).collect();
        pisteet.sort_by_key(|p| p.0);
        if pisteet.is_empty() {
            bail!("The project has no keyframes.");
        }
        loki(&format!("Keyframes {}:", pisteet.len()));
        for (f, r) in &pisteet {
            loki(&format!("  {}  (frame {f})  {}", aikateksti(self.t.aika(*f)), r.kuvaus()));
        }
        let mut tarvitaan: Vec<String> = pisteet.iter().flat_map(|(_, r)| r.mallit()).collect();
        tarvitaan.sort();
        tarvitaan.dedup();
        for m in &tarvitaan {
            if !mallit::on_ladattu(m) {
                bail!("Model {m} is missing. Install it first.");
            }
        }
        loki("Finding scene cuts…");
        let rajat = kohtausrajat(&self.t, self.a.leikkaus_raja, self.alku, self.maara, &self.seis)?;
        loki(&format!("Scenes: {}", rajat.len() + 1));
        for m in &tarvitaan {
            mallit::hae(m, &self.a.laite)?;
        }
        let kehykset: Vec<usize> = pisteet.iter().map(|p| p.0).collect();
        let kohtaus = |f: usize| rajat.partition_point(|&r| r <= f);
        let mut reunat = vec![self.alku];
        reunat.extend(&rajat);
        reunat.push(self.alku + self.maara);
        let keski: Vec<f64> = reunat.windows(2).map(|w| (w[0] + w[1]) as f64 / 2.0).collect();
        let painot = |f: usize| -> (usize, Option<usize>, f32) {
            if f <= kehykset[0] {
                return (0, None, 0.0);
            }
            if f >= *kehykset.last().unwrap() {
                return (kehykset.len() - 1, None, 0.0);
            }
            let k = kehykset.partition_point(|&x| x <= f) - 1;
            let (f1, f2) = (kehykset[k], kehykset[k + 1]);
            let (s1, s2, s) = (kohtaus(f1), kohtaus(f2), kohtaus(f));
            let x = if s1 == s2 {
                (f - f1) as f64 / (f2 - f1).max(1) as f64
            } else if s == s1 {
                0.0
            } else if s == s2 {
                1.0
            } else {
                (keski[s] - f1 as f64) / (f2 - f1).max(1) as f64
            };
            let w = s_kayra(x as f32);
            if w <= 0.0 {
                (k, None, 0.0)
            } else if w >= 1.0 {
                (k + 1, None, 0.0)
            } else {
                (k, Some(k + 1), w)
            }
        };
        let rajat_set: std::collections::HashSet<usize> = rajat.iter().copied().collect();
        let kirj = Kirjoittaja::avaa(&self.a.ulos, &self.t, &self.kirjoitusasetukset(None))?;
        let mut jk = Jalkikasittely::kaynnista(&self.a, &self.t, kirj, self.seis.clone());
        let rx = lukijasaie(&self.t, self.alku, self.maara, None)?;
        let mut ed = Edistys::new("Render", self.maara);
        let mut i = 0;
        let tulos = (|| -> Result<()> {
            loop {
                self.tarkista_seis()?;
                let era: Vec<Bgr> = rx.iter().take(ERA).collect();
                if era.is_empty() {
                    return Ok(());
                }
                let pienet: Vec<Bgr> = era.iter().map(|k| k.koon(self.ww, self.wh)).collect();
                let suunnitelma: Vec<_> = (0..era.len()).map(|j| painot(self.alku + i + j)).collect();
                let mut mab: Vec<HashMap<String, Kentta>> = vec![HashMap::new(); era.len()];
                for m in &tarvitaan {
                    let ind: Vec<usize> = (0..era.len())
                        .filter(|&j| {
                            let (a, b, _) = suunnitelma[j];
                            pisteet[a].1.mallit().contains(m) || b.map(|b| pisteet[b].1.mallit().contains(m)).unwrap_or(false)
                        })
                        .collect();
                    if ind.is_empty() {
                        continue;
                    }
                    let kuvat: Vec<&Bgr> = ind.iter().map(|&j| &pienet[j]).collect();
                    let malli = mallit::hae(m, &self.a.laite)?;
                    let tulos = malli.lock().unwrap().ab(&kuvat)?;
                    for (j, ab) in ind.into_iter().zip(tulos) {
                        mab[j].insert(m.clone(), ab);
                    }
                }
                for (j, k) in era.iter().enumerate() {
                    let f = self.alku + i + j;
                    let (a, b, w) = suunnitelma[j];
                    let l_full = l_kanava(k);
                    let lw = l_full.koon(self.ww, self.wh);
                    let (_, mut ab) = sovella(&pisteet[a].1, &lw, &mab[j], &self.refit)?;
                    let mut v = pisteet[a].1.v;
                    if let Some(b) = b {
                        let (_, ab2) = sovella(&pisteet[b].1, &lw, &mab[j], &self.refit)?;
                        ab = lch_sekoitus(&ab, &ab2, w);
                        v = (1.0 - w) * v + w * pisteet[b].1.v;
                    }
                    jk.syota((l_full, lw, ab, v, rajat_set.contains(&f)))?;
                }
                i += era.len();
                ed.paivita(i);
            }
        })();
        if let Err(e) = tulos {
            self.seis.store(true, Ordering::Relaxed);
            let _ = jk.lopeta();
            self.seis.store(false, Ordering::Relaxed);
            return Err(e);
        }
        let n = jk.lopeta()?;
        ed.valmis();
        Ok(n)
    }
}

/// Pienoiskuva (96 px leveä harmaa) kopioruutujen tunnistukseen.
fn kopiotunniste(k: &Bgr) -> Vec<u8> {
    let h = ((k.h as f64 * 96.0 / k.w as f64).round() as usize).max(2);
    k.koon(96, h).harmaa()
}

/// Kuvat ovat käytännössä samat (keskimääräinen ero alle 0,35 harmaasävyä 0–255; pakkauskohinaa suurempi
/// ero ei ole kopio). Kirkkaus otetaan aina kuvasta itsestään, joten väärä osuma ei näy.
fn on_kopio(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs()).sum::<u32>() as f32 / (a.len() as f32) < 0.35
}

/// Värittämättömät läikät: harmaata aluetta, jota ympäröi selvä väri. Palauttaa pehmeän maskin.
pub fn laikut(l: &Taso, c: &Taso) -> Option<Taso> {
    let (w, h) = (l.w, l.h);
    let harmaa: Vec<u8> = (0..w * h).map(|i| (c.data[i] < 2.5 && l.data[i] > 15.0 && l.data[i] < 92.0) as u8).collect();
    let auki = laajenna(&laajenna(&harmaa, w, h, 3, false), w, h, 3, true);
    let (nimio, lista) = alueet(&auki, w, h);
    let raja = (0.004 * (w * h) as f32) as usize;
    let mut maski = vec![0u8; w * h];
    let mut loytyi = false;
    for (k, a) in lista.iter().enumerate() {
        if a.pinta < raja || a.pinta as f32 > 0.3 * (w * h) as f32 {
            continue;
        }
        let n = k as u32 + 1;
        let (x0, y0, x1, y1) = (a.x0.saturating_sub(10), a.y0.saturating_sub(10), (a.x1 + 11).min(w), (a.y1 + 11).min(h));
        let (bw, bh) = (x1 - x0, y1 - y0);
        let alue: Vec<u8> = (0..bw * bh).map(|i| (nimio[(y0 + i / bw) * w + x0 + i % bw] == n) as u8).collect();
        let laaja = laajenna(&alue, bw, bh, 15, true);
        let (mut s, mut m) = (0.0f32, 0usize);
        for i in 0..bw * bh {
            if laaja[i] != 0 && alue[i] == 0 {
                s += c.data[(y0 + i / bw) * w + x0 + i % bw];
                m += 1;
            }
        }
        if m > 0 && s / m as f32 > 10.0 {
            for i in 0..bw * bh {
                if alue[i] != 0 {
                    maski[(y0 + i / bw) * w + x0 + i % bw] = 1;
                }
            }
            loytyi = true;
        }
    }
    if !loytyi {
        return None;
    }
    let laaja = laajenna(&maski, w, h, 7, true);
    Some(Taso::u8sta(w, h, &laaja).gauss(3.0))
}

/// Kehysindeksit, joista uusi kohtaus alkaa (nopea ajo pienessä koossa).
pub fn kohtausrajat(t: &Tiedot, raja: f32, alku: usize, maara: usize, seis: &AtomicBool) -> Result<Vec<usize>> {
    let (w, h) = t.koko_enintaan(320);
    let mut tunnistin = Leikkaukset::new(raja);
    let mut rajat = Vec::new();
    let n = if maara > 0 { maara } else { t.n.saturating_sub(alku) };
    let mut ed = Edistys::new("Scene cuts", n);
    for (i, k) in Lukija::avaa(t, alku, maara, Some((w, h)))?.enumerate() {
        if i % 256 == 0 && seis.load(Ordering::Relaxed) {
            bail!("Cancelled.");
        }
        if tunnistin.onko(&Taso::u8sta(w, h, &k.harmaa())) {
            rajat.push(alku + i);
        }
        ed.paivita(i + 1);
    }
    Ok(rajat)
}

/// Kiinnipiste pisimpien kohtausten keskelle (enintään `enintaan`, aina myös ensimmäinen).
pub fn ehdota_kiinnipisteet(rajat: &[usize], n_kuvia: usize, enintaan: usize) -> Vec<usize> {
    let mut reunat = vec![0];
    reunat.extend(rajat);
    reunat.push(n_kuvia);
    let kohtaukset: Vec<(usize, usize)> = reunat.windows(2).filter(|w| w[1] > w[0]).map(|w| (w[0], w[1])).collect();
    if kohtaukset.is_empty() {
        return vec![];
    }
    let mut muut: Vec<(usize, usize)> = kohtaukset[1..].to_vec();
    muut.sort_by_key(|k| std::cmp::Reverse(k.1 - k.0));
    muut.truncate(enintaan.saturating_sub(1));
    let mut v: Vec<usize> = std::iter::once(kohtaukset[0]).chain(muut).map(|(a, b)| (a + b) / 2).collect();
    v.sort();
    v
}

/// Ajaa reseptin yhdelle (esikatselukokoiselle) kehykselle. `ab_muisti` säilyttää mallien tulokset.
pub fn aja_resepti(r: &Resepti, k: &Bgr, ab_muisti: &mut HashMap<String, Kentta>, laite: &str, refit: &Referenssit) -> Result<(Taso, Kentta)> {
    let l = l_kanava(k);
    for m in r.mallit() {
        if !ab_muisti.contains_key(&m) {
            if !mallit::on_ladattu(&m) {
                bail!("Model {m} is missing – install it on the Add-ons tab.");
            }
            let malli = mallit::hae(&m, laite)?;
            let ab = malli.lock().unwrap().ab(&[k])?.remove(0);
            ab_muisti.insert(m, ab);
        }
    }
    sovella(r, &l, ab_muisti, refit)
}
