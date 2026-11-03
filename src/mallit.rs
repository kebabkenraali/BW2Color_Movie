//! Väritysmallit ONNX Runtimellä (DirectML-näytönohjain, CPU varalla). Mallit palauttavat vain a/b-kanavat;
//! kirkkaus otetaan aina alkuperäisestä kuvasta.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};
use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;

use crate::kuva::{Bgr, Kentta, Taso};
use crate::vari::{harmaa_rgb_tasot, l_kanava};
use crate::video::sovelluskansio;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tyyppi {
    DDColor,
}

pub struct MalliTieto {
    pub nimi: &'static str,
    pub kuvaus: &'static str,
    pub tyyppi: Tyyppi,
    pub tiedosto: &'static str,
    pub url: Option<&'static str>,
    pub koko_mt: u32,
}

const FF: &str = "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0/";

pub const MALLIT: &[MalliTieto] = &[
    MalliTieto { nimi: "ddcolor_suomi", kuvaus: "DDColor Suomi (fine-tuned on 1950s–80s colour film, automatic style)", tyyppi: Tyyppi::DDColor,
                 tiedosto: "ddcolor_suomi/ddcolor_suomi.onnx", url: None, koko_mt: 934 },
];

/// ddcolor_suomen filmityylit: sama mallitiedosto, toinen syöte "tyyli" (one-hot, indeksi 1..). Tyyli näkyy
/// omana mallinaan, joten se toimii kaikkialla, missä mallin voi valita (Väritä, Exemplar, putki, komentorivi).
const TYYLIPOHJA: &str = "ddcolor_suomi";
pub const TYYLIT: &[(&str, &str)] = &[
    ("ddcolor_suomi_technicolor", "DDColor Suomi – Technicolor (saturated 1950s feature film)"),
    ("ddcolor_suomi_eastmancolor", "DDColor Suomi – Eastmancolor (1960s–70s film stock)"),
    ("ddcolor_suomi_kodachrome", "DDColor Suomi – Kodachrome (bright, crisp amateur film)"),
    ("ddcolor_suomi_moderni", "DDColor Suomi – modern (21st-century colours)"),
];
const TYYLEJA: usize = TYYLIT.len() + 1;

/// (mallitiedoston nimi, tyyli-indeksi; 0 = automaattinen)
fn jaa(nimi: &str) -> (&str, usize) {
    match TYYLIT.iter().position(|(n, _)| *n == nimi) {
        Some(i) => (TYYLIPOHJA, i + 1),
        None => (nimi, 0),
    }
}

pub fn tieto(nimi: &str) -> Option<&'static MalliTieto> {
    let pohja = jaa(nimi).0;
    MALLIT.iter().find(|m| m.nimi == pohja)
}

pub fn kuvaus(nimi: &str) -> Option<String> {
    TYYLIT.iter().find(|(n, _)| *n == nimi).map(|(_, k)| k.to_string()).or_else(|| tieto(nimi).map(|t| t.kuvaus.to_string()))
}

/// Kaikki valittavat mallit (Väritä-välilehden menetelmä): asennetut ja asentamattomat, filmityylit pohjamallin jälkeen.
pub fn valittavat() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = MALLIT.iter().map(|m| m.nimi).collect();
    if on_ladattu(TYYLIPOHJA) {
        let i = v.iter().position(|m| *m == TYYLIPOHJA).map_or(v.len(), |i| i + 1);
        v.splice(i..i, TYYLIT.iter().map(|(n, _)| *n));
    }
    v
}

pub fn ddcolor_mallit() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = MALLIT.iter().filter(|m| m.tyyppi == Tyyppi::DDColor).map(|m| m.nimi).collect();
    if on_ladattu(TYYLIPOHJA) {
        let i = v.iter().position(|m| *m == TYYLIPOHJA).map_or(v.len(), |i| i + 1);
        v.splice(i..i, TYYLIT.iter().map(|(n, _)| *n));
    }
    v
}

pub fn mallikansio() -> PathBuf {
    sovelluskansio().join("models")
}

pub fn polku(nimi: &str) -> Option<PathBuf> {
    let t = tieto(nimi)?;
    let p = mallikansio().join(t.tiedosto);
    (p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)).then_some(p)
}

pub fn on_ladattu(nimi: &str) -> bool {
    polku(nimi).is_some()
}

/// Lataa mallin verkosta ja tarkistaa sen julkaisijan CRC32-tarkisteella (.hash). `edistys(saatu, koko)` tavuina.
pub fn lataa(nimi: &str, edistys: &mut dyn FnMut(u64, u64), seis: &std::sync::atomic::AtomicBool) -> Result<PathBuf> {
    let t = tieto(nimi).ok_or_else(|| anyhow!("unknown model {nimi}"))?;
    let tiedosto = t.url.ok_or_else(|| anyhow!("Model {nimi} cannot be downloaded – import it from a file (Add-ons)."))?;
    let kohde = mallikansio().join(t.tiedosto);
    if kohde.is_file() {
        return Ok(kohde);
    }
    let odotettu = ureq::get(&format!("{FF}{}", tiedosto.replace(".onnx", ".hash")))
        .call()
        .ok()
        .and_then(|mut v| v.body_mut().read_to_string().ok())
        .map(|s| s.trim().to_lowercase());
    let crc = crate::lisaosat::lataa_tiedosto(&format!("{FF}{tiedosto}"), &kohde, edistys, seis)?;
    if let Some(h) = odotettu.filter(|h| h.len() == 8) {
        if format!("{crc:08x}") != h {
            let _ = std::fs::remove_file(&kohde);
            bail!("Checksum of model {nimi} does not match ({crc:08x} ≠ {h}) – the download is corrupted, try again.");
        }
        std::fs::write(kohde.with_extension("hash"), &h)?;
    }
    Ok(kohde)
}

/// Tuo mallitiedoston (.onnx) levyltä, esim. toisesta asennuksesta (Paper/Tiny eivät ole ladattavissa verkosta).
pub fn tuo(nimi: &str, lahde: &std::path::Path) -> Result<PathBuf> {
    let t = tieto(nimi).ok_or_else(|| anyhow!("unknown model {nimi}"))?;
    let mut otsikko = [0u8; 2];
    std::fs::File::open(lahde)?.read_exact(&mut otsikko)?;
    if otsikko[0] != 0x08 {
        bail!("{} does not look like an ONNX model.", lahde.display());
    }
    let kohde = mallikansio().join(t.tiedosto);
    std::fs::create_dir_all(kohde.parent().unwrap())?;
    std::fs::copy(lahde, &kohde)?;
    // tarkistetaan, että malli avautuu
    if let Err(e) = Malli::lataa(nimi, "cpu") {
        let _ = std::fs::remove_file(&kohde);
        bail!("Could not open the model: {e:#}");
    }
    Ok(kohde)
}

pub fn poista(nimi: &str) -> Result<()> {
    if let Some(p) = polku(nimi) {
        vapauta();
        std::fs::remove_file(&p)?;
        let _ = std::fs::remove_file(p.with_extension("hash"));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Istunnot
// --------------------------------------------------------------------------
fn ymparisto() -> Result<&'static ort::environment::Environment> {
    static E: OnceLock<std::result::Result<ort::environment::Environment, String>> = OnceLock::new();
    E.get_or_init(|| ort::init().with_name("varita").build().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow!("ONNX Runtime does not start: {e}"))
}

pub struct Malli {
    istunto: Session,
    koko: (usize, usize),
    era: usize,
    pub laite: String,
    /// filmityyli, jos malli ottaa sen syötteenä
    tyyli: Option<usize>,
}

pub(crate) fn luo_istunto(polku: &PathBuf, gpu: bool) -> Result<Session> {
    let mut b = Session::builder(ymparisto()?)?;
    if gpu {
        b = b
            .with_memory_pattern(false)
            .map_err(|e| anyhow!(e.message().to_string()))?
            .with_parallel_execution(false)
            .map_err(|e| anyhow!(e.message().to_string()))?
            .with_execution_providers([DirectML::default().build().error_on_failure()])
            .map_err(|e| anyhow!(e.message().to_string()))?;
    }
    Ok(b.commit_from_file(polku)?)
}

impl Malli {
    pub fn lataa(nimi: &str, laite: &str) -> Result<Malli> {
        tieto(nimi).ok_or_else(|| anyhow!("unknown model {nimi}"))?;
        let p = polku(nimi).ok_or_else(|| anyhow!("Model {nimi} is not installed (Add-ons tab)."))?;
        let (istunto, kaytossa) = if laite == "cpu" {
            (luo_istunto(&p, false)?, "CPU")
        } else {
            match luo_istunto(&p, true) {
                Ok(s) => (s, "GPU (DirectML)"),
                Err(e) => {
                    crate::loki(&format!("{nimi}: graphics card not available ({e}), using the CPU."));
                    (luo_istunto(&p, false)?, "CPU")
                }
            }
        };
        let syote = &istunto.inputs()[0];
        let muoto: Vec<i64> = match syote.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => shape.iter().copied().collect(),
            _ => vec![-1, 3, -1, -1],
        };
        let koko = if muoto.len() == 4 && muoto[2] > 0 && muoto[3] > 0 { (muoto[3] as usize, muoto[2] as usize) } else { (512, 512) };
        let era = if muoto.first().copied().unwrap_or(-1) > 0 { muoto[0] as usize } else if kaytossa == "CPU" { 1 } else { 4 };
        let pyydetty = jaa(nimi).1;
        let tyyli = (istunto.inputs().len() >= 2).then_some(pyydetty);
        if tyyli.is_none() && pyydetty > 0 {
            crate::loki(&format!("{nimi}: the installed {TYYLIPOHJA} does not support film styles (old version) – using automatic."));
        }
        Ok(Malli { istunto, koko, era, laite: kaytossa.to_string(), tyyli })
    }

    /// Kuvat (BGR) -> a/b-kentät samassa koossa.
    pub fn ab(&mut self, kuvat: &[&Bgr]) -> Result<Vec<Kentta>> {
        let mut tulos = Vec::with_capacity(kuvat.len());
        let (sw, sh) = self.koko;
        for era in kuvat.chunks(self.era.max(1)) {
            let mut syote = Vec::with_capacity(era.len() * 3 * sw * sh);
            for k in era {
                syote.extend(harmaa_rgb_tasot(&l_kanava(k).koon(sw, sh)));
            }
            let x = Tensor::from_array(([era.len(), 3, sh, sw], syote))?;
            let ulos = match self.tyyli {
                Some(s) => {
                    let mut yksi = vec![0f32; era.len() * TYYLEJA];
                    for j in 0..era.len() {
                        yksi[j * TYYLEJA + s] = 1.0;
                    }
                    let ty = Tensor::from_array(([era.len(), TYYLEJA], yksi))?;
                    self.istunto.run(ort::inputs![x, ty])?
                }
                None => self.istunto.run(ort::inputs![x])?,
            };
            let (muoto, arvot) = ulos[0].try_extract_tensor::<f32>()?;
            let kanavia = muoto[1] as usize;
            let (oh, ow) = (muoto[2] as usize, muoto[3] as usize);
            let n = oh * ow;
            for (j, k) in era.iter().enumerate() {
                let alku = j * kanavia * n;
                let ab = Kentta {
                    x: Taso { w: ow, h: oh, data: arvot[alku..alku + n].to_vec() },
                    y: Taso { w: ow, h: oh, data: arvot[alku + n..alku + 2 * n].to_vec() },
                };
                tulos.push(ab.koon(k.w, k.h));
            }
        }
        Ok(tulos)
    }
}

type Jaettu = Arc<Mutex<Malli>>;

fn valimuisti() -> &'static Mutex<HashMap<(String, String), Jaettu>> {
    static V: OnceLock<Mutex<HashMap<(String, String), Jaettu>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ladattu malli välimuistista (yksi istunto kutakin mallia ja laitetta kohden).
pub fn hae(nimi: &str, laite: &str) -> Result<Jaettu> {
    let avain = (nimi.to_string(), laite.to_string());
    if let Some(m) = valimuisti().lock().unwrap().get(&avain) {
        return Ok(m.clone());
    }
    crate::loki(&format!("Loading model {nimi}…"));
    let m = Arc::new(Mutex::new(Malli::lataa(nimi, laite)?));
    crate::loki(&format!("{nimi}: {}", m.lock().unwrap().laite));
    valimuisti().lock().unwrap().insert(avain, m.clone());
    Ok(m)
}

/// Vapauttaa ladatut mallit (näytönohjaimen muisti), esim. ennen näkömallin ajoa.
pub fn vapauta() {
    valimuisti().lock().unwrap().clear();
}
