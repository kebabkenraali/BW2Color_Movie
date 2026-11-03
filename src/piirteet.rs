//! DINOv2-small (Meta, Apache-2.0): kuvan piirteet 14 px:n ruuduittain. Käytetään kohtausmuistiin:
//! saman kohteen tunnistamiseen eri kuvakulmista leikkausten yli. Syötteenä aina mustavalkoinen kirkkaus,
//! joten vertailu ei riipu väreistä.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};
use ort::session::Session;
use ort::value::Tensor;

use crate::kuva::Taso;

pub const NIMI: &str = "dinov2_small";
pub const KUVAUS: &str = "DINOv2-small: scene memory (same object keeps its colour across cuts)";
pub const LISENSSI: &str = "Apache-2.0 (DINOv2, Meta)";
pub const KOKO_MT: u32 = 85;
const URL: &str = "https://huggingface.co/onnx-community/dinov2-small/resolve/main/onnx/model.onnx";
const TIEDOSTO: &str = "dinov2_small/dinov2_small.onnx";
/// Tarkiste lasketaan latauksen aikana; vastaa julkaisijan SHA-256:ta f22797ea…250c20a.
const CRC32: u32 = 0x1b20_4732;
/// Syötteen leveys (32 ruutua); korkeus kuvasuhteen mukaan 14:n monikertana.
const LEVEYS: usize = 448;
pub const ULOTTUVUUS: usize = 384;

pub fn polku() -> Option<PathBuf> {
    let p = crate::mallit::mallikansio().join(TIEDOSTO);
    (p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)).then_some(p)
}

pub fn on_ladattu() -> bool {
    polku().is_some()
}

pub fn lataa(edistys: &mut dyn FnMut(u64, u64), seis: &AtomicBool) -> Result<PathBuf> {
    let kohde = crate::mallit::mallikansio().join(TIEDOSTO);
    if kohde.is_file() {
        return Ok(kohde);
    }
    let crc = crate::lisaosat::lataa_tiedosto(URL, &kohde, edistys, seis)?;
    if crc != CRC32 {
        let _ = std::fs::remove_file(&kohde);
        bail!("Checksum of the DINOv2 model does not match ({crc:08x} ≠ {CRC32:08x}) – the download is corrupted, try again.");
    }
    Ok(kohde)
}

pub fn poista() -> Result<()> {
    if let Some(p) = polku() {
        vapauta();
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// Yhden kuvan piirteet: `p` = ruutujen piirteet (gw*gh*384, normeerattu yksikköpituisiksi), `cls` = koko kuva.
pub struct Piirteet {
    pub gw: usize,
    pub gh: usize,
    pub cls: Vec<f32>,
    pub p: Vec<f32>,
}

impl Piirteet {
    pub fn ruutu(&self, i: usize) -> &[f32] {
        &self.p[i * ULOTTUVUUS..(i + 1) * ULOTTUVUUS]
    }
    pub fn n(&self) -> usize {
        self.gw * self.gh
    }
}

pub fn pistetulo(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normeeraa(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= n);
}

pub struct Dino {
    istunto: Session,
}

impl Dino {
    fn lataa(laite: &str) -> Result<Dino> {
        let p = polku().ok_or_else(|| anyhow!("The DINOv2 model is not installed (Add-ons)."))?;
        let istunto = if laite == "cpu" {
            crate::mallit::luo_istunto(&p, false)?
        } else {
            crate::mallit::luo_istunto(&p, true).or_else(|_| crate::mallit::luo_istunto(&p, false))?
        };
        Ok(Dino { istunto })
    }

    /// Harmaakuva (0..255) -> piirteet.
    pub fn piirteet(&mut self, g: &Taso) -> Result<Piirteet> {
        let gw = LEVEYS / 14;
        let gh = (((g.h as f32 * LEVEYS as f32 / g.w as f32) / 14.0).round() as usize).max(1);
        let (w, h) = (gw * 14, gh * 14);
        let k = g.koon(w, h);
        let mut syote = Vec::with_capacity(3 * w * h);
        for (ka, kh) in [(0.485f32, 0.229f32), (0.456, 0.224), (0.406, 0.225)] {
            syote.extend(k.data.iter().map(|v| (v / 255.0 - ka) / kh));
        }
        let x = Tensor::from_array(([1usize, 3, h, w], syote))?;
        let ulos = self.istunto.run(ort::inputs![x])?;
        let (muoto, arvot) = ulos[0].try_extract_tensor::<f32>()?;
        let tokeneita = muoto[1] as usize;
        if tokeneita != gw * gh + 1 || muoto[2] as usize != ULOTTUVUUS {
            bail!("DINOv2: unexpected output shape {muoto:?}");
        }
        let mut cls = arvot[..ULOTTUVUUS].to_vec();
        normeeraa(&mut cls);
        let mut p = arvot[ULOTTUVUUS..tokeneita * ULOTTUVUUS].to_vec();
        p.chunks_mut(ULOTTUVUUS).for_each(normeeraa);
        Ok(Piirteet { gw, gh, cls, p })
    }
}

fn valimuisti() -> &'static Mutex<Option<(String, Arc<Mutex<Dino>>)>> {
    static V: OnceLock<Mutex<Option<(String, Arc<Mutex<Dino>>)>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(None))
}

/// Ladattu DINOv2 (yksi istunto).
pub fn hae(laite: &str) -> Result<Arc<Mutex<Dino>>> {
    let mut v = valimuisti().lock().unwrap();
    if let Some((l, d)) = v.as_ref() {
        if l == laite {
            return Ok(d.clone());
        }
    }
    let d = Arc::new(Mutex::new(Dino::lataa(laite)?));
    *v = Some((laite.to_string(), d.clone()));
    Ok(d)
}

pub fn vapauta() {
    *valimuisti().lock().unwrap() = None;
}
