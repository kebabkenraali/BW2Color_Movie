//! Värittämättömien kohtien tunnistin (oma malli, koulutus: koulutus/tyokalut/tunnistin_kouluta.py):
//! pieni U-Net saa väritetyn kuvan (L, a, b) ja antaa todennäköisyyden kohdille, jotka olisi pitänyt
//! värittää mutta jäivät harmaiksi – kasvot, ihminen tai esine muuten värillisessä kuvassa.
//! Löydöksistä hylätään hyvin tummat kohdat (väriä ei voi arvioida), kuvan reunaan ulottuvat alueet
//! (taivas, seinä ja katu saavat olla harmaita) sekä hyvin pienet alueet.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use ort::session::Session;
use ort::value::Tensor;

use crate::kuva::{Kentta, Taso, alueet, laajenna};

const TIEDOSTO: &str = "tunnistin/varittamaton.onnx";
/// Tunnistin ajetaan enintään tämän levyisenä (nopeus); maski skaalataan takaisin.
const LEVEYS: usize = 512;
const REUNA: usize = 16;
const KYNNYS: f32 = 0.65;
const TUMMA: f32 = 15.0;

pub fn polku() -> Option<PathBuf> {
    let p = crate::mallit::mallikansio().join(TIEDOSTO);
    p.is_file().then_some(p)
}

pub struct Tunnistin {
    istunto: Session,
}

/// Peilaava indeksi välille 0..n.
fn peili(i: i64, n: usize) -> usize {
    let n = n as i64;
    let mut i = i;
    if i < 0 {
        i = -i;
    }
    if i >= n {
        i = 2 * (n - 1) - i;
    }
    i.clamp(0, n - 1) as usize
}

impl Tunnistin {
    pub fn lataa(laite: &str) -> Result<Tunnistin> {
        let p = polku().ok_or_else(|| anyhow!("detector model not found ({TIEDOSTO})"))?;
        let istunto = if laite == "cpu" {
            crate::mallit::luo_istunto(&p, false)?
        } else {
            crate::mallit::luo_istunto(&p, true).or_else(|_| crate::mallit::luo_istunto(&p, false))?
        };
        Ok(Tunnistin { istunto })
    }

    /// `l` = kirkkaus (0..100), `ab` = väritys samassa koossa. Palauttaa pehmeän maskin (0..1) samassa
    /// koossa, tai None, jos värittämättömiä kohtia ei löytynyt.
    pub fn maski(&mut self, l: &Taso, ab: &Kentta) -> Result<Option<Taso>> {
        let (w0, h0) = (l.w, l.h);
        let s = (LEVEYS as f32 / w0 as f32).min(1.0);
        let (w, h) = (((w0 as f32 * s) as usize).max(16), ((h0 as f32 * s) as usize).max(16));
        let (lp, abp) = (l.koon(w, h), ab.koon(w, h));
        let (sw, sh) = ((w + 2 * REUNA).div_ceil(16) * 16, (h + 2 * REUNA).div_ceil(16) * 16);
        let mut syote = vec![0.0f32; 3 * sw * sh];
        for y in 0..sh {
            let yy = peili(y as i64 - REUNA as i64, h);
            for x in 0..sw {
                let xx = peili(x as i64 - REUNA as i64, w);
                let i = yy * w + xx;
                let o = y * sw + x;
                syote[o] = lp.data[i] / 100.0;
                syote[sw * sh + o] = abp.x.data[i] / 100.0;
                syote[2 * sw * sh + o] = abp.y.data[i] / 100.0;
            }
        }
        let x = Tensor::from_array(([1usize, 3, sh, sw], syote))?;
        let ulos = self.istunto.run(ort::inputs![x])?;
        let (_, arvot) = ulos[0].try_extract_tensor::<f32>()?;
        let mut binaari = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let p = arvot[(y + REUNA) * sw + x + REUNA];
                binaari[y * w + x] = (p > KYNNYS && lp.data[y * w + x] > TUMMA) as u8;
            }
        }
        let (nimio, lista) = alueet(&binaari, w, h);
        let pienin = (0.004 * (w * h) as f32) as usize;
        let mut maski = vec![0u8; w * h];
        let mut loytyi = false;
        for (k, a) in lista.iter().enumerate() {
            let reunalla = a.x0 == 0 || a.y0 == 0 || a.x1 + 1 >= w || a.y1 + 1 >= h;
            if reunalla || a.pinta < pienin {
                continue;
            }
            let n = k as u32 + 1;
            for (i, &v) in nimio.iter().enumerate() {
                if v == n {
                    maski[i] = 1;
                }
            }
            loytyi = true;
        }
        if !loytyi {
            return Ok(None);
        }
        let laaja = laajenna(&maski, w, h, 5, true);
        Ok(Some(Taso::u8sta(w, h, &laaja).gauss(2.0).koon(w0, h0)))
    }
}
