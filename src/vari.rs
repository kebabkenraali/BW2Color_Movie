//! Väriavaruus: sRGB <-> CIE Lab (D65, sama kuin OpenCV:n float-muunnos), referenssin värien siirto,
//! käsisäätö (H/S/V Labissa), LCh-interpolointi ja S-käyrä.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::kuva::{Bgr, Kentta, Taso};

const XN: f32 = 0.950456;
const ZN: f32 = 1.088754;

fn gamma_lut() -> &'static [f32; 256] {
    static L: OnceLock<[f32; 256]> = OnceLock::new();
    L.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

const KAANT_N: usize = 8192;
fn kaant_lut() -> &'static Vec<f32> {
    static L: OnceLock<Vec<f32>> = OnceLock::new();
    L.get_or_init(|| {
        (0..=KAANT_N)
            .map(|i| {
                let c = i as f32 / KAANT_N as f32;
                let v = if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
                v * 255.0
            })
            .collect()
    })
}

#[inline]
fn lin_srgb(c: f32) -> f32 {
    // lineaarinen 0..1 -> sRGB 0..255; tiheä taulukko + lineaarinen interpolointi
    let c = c.clamp(0.0, 1.0) * KAANT_N as f32;
    let i = (c as usize).min(KAANT_N - 1);
    let f = c - i as f32;
    let t = kaant_lut();
    t[i] * (1.0 - f) + t[i + 1] * f
}

#[inline]
fn f_lab(t: f32) -> f32 {
    if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
}

#[inline]
fn f_lab_inv(t: f32) -> f32 {
    let t3 = t * t * t;
    if t3 > 0.008856 { t3 } else { (t - 16.0 / 116.0) / 7.787 }
}

/// Lab-kuva: L 0..100 ja a/b-kenttä.
#[derive(Clone)]
pub struct Lab {
    pub l: Taso,
    pub ab: Kentta,
}

pub fn bgr_lab(k: &Bgr) -> Lab {
    let g = gamma_lut();
    let n = k.w * k.h;
    let mut l = vec![0.0f32; n];
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    l.par_iter_mut()
        .zip(a.par_iter_mut())
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((l, a), b))| {
            let p = &k.data[i * 3..i * 3 + 3];
            let (bb, gg, rr) = (g[p[0] as usize], g[p[1] as usize], g[p[2] as usize]);
            let x = (0.412453 * rr + 0.357580 * gg + 0.180423 * bb) / XN;
            let y = 0.212671 * rr + 0.715160 * gg + 0.072169 * bb;
            let z = (0.019334 * rr + 0.119193 * gg + 0.950227 * bb) / ZN;
            let (fx, fy, fz) = (f_lab(x), f_lab(y), f_lab(z));
            *l = if y > 0.008856 { 116.0 * fy - 16.0 } else { 903.3 * y };
            *a = 500.0 * (fx - fy);
            *b = 200.0 * (fy - fz);
        });
    Lab {
        l: Taso { w: k.w, h: k.h, data: l },
        ab: Kentta { x: Taso { w: k.w, h: k.h, data: a }, y: Taso { w: k.w, h: k.h, data: b } },
    }
}

pub fn l_kanava(k: &Bgr) -> Taso {
    bgr_lab(k).l
}

#[inline]
fn lab_rgb_lin(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let y = if l > 7.9996 { fy * fy * fy } else { l / 903.3 };
    let x = f_lab_inv(fx) * XN;
    let z = f_lab_inv(fz) * ZN;
    (
        3.240479 * x - 1.53715 * y - 0.498535 * z,
        -0.969256 * x + 1.875991 * y + 0.041556 * z,
        0.055648 * x - 0.204043 * y + 1.057311 * z,
    )
}

pub fn lab_bgr(l: &Taso, ab: &Kentta) -> Bgr {
    let (w, h) = (l.w, l.h);
    let mut ulos = Bgr::new(w, h);
    ulos.data.par_chunks_mut(3).enumerate().for_each(|(i, p)| {
        let (r, g, b) = lab_rgb_lin(l.data[i], ab.x.data[i], ab.y.data[i]);
        p[0] = lin_srgb(b).round() as u8;
        p[1] = lin_srgb(g).round() as u8;
        p[2] = lin_srgb(r).round() as u8;
    });
    ulos
}

/// Pelkkä kirkkaus RGB-muodossa (a = b = 0), 0..1, mallin syötteeksi tasomuodossa [R, G, B].
pub fn harmaa_rgb_tasot(l: &Taso) -> Vec<f32> {
    let n = l.w * l.h;
    let mut v = vec![0.0f32; 3 * n];
    let (r, gb) = v.split_at_mut(n);
    let (g, b) = gb.split_at_mut(n);
    r.par_iter_mut().zip(g.par_iter_mut()).zip(b.par_iter_mut()).enumerate().for_each(|(i, ((r, g), b))| {
        let (rr, gg, bb) = lab_rgb_lin(l.data[i], 0.0, 0.0);
        *r = lin_srgb(rr) / 255.0;
        *g = lin_srgb(gg) / 255.0;
        *b = lin_srgb(bb) / 255.0;
    });
    v
}

pub fn kroma(ab: &Kentta) -> Taso {
    Taso {
        w: ab.w(),
        h: ab.h(),
        data: ab.x.data.par_iter().zip(&ab.y.data).map(|(a, b)| a.hypot(*b)).collect(),
    }
}

// --------------------------------------------------------------------------
// S-käyrä ja LCh-interpolointi
// --------------------------------------------------------------------------
pub fn s_kayra(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// a/b-kenttien interpolointi: kroma lineaarisesti, sävy vektorien suunnasta (ei harmaantumista).
pub fn lch_sekoitus(ab1: &Kentta, ab2: &Kentta, w: f32) -> Kentta {
    if w <= 0.0 {
        return ab1.clone();
    }
    if w >= 1.0 {
        return ab2.clone();
    }
    let mut ulos = ab1.clone();
    ulos.x.data.par_iter_mut().zip(ulos.y.data.par_iter_mut()).enumerate().for_each(|(i, (a, b))| {
        let (a1, b1, a2, b2) = (ab1.x.data[i], ab1.y.data[i], ab2.x.data[i], ab2.y.data[i]);
        let (aa, bb) = ((1.0 - w) * a1 + w * a2, (1.0 - w) * b1 + w * b2);
        let c = aa.hypot(bb);
        let tavoite = (1.0 - w) * a1.hypot(b1) + w * a2.hypot(b2);
        let k = if c < 0.5 { 1.0 } else { (tavoite / c.max(1e-3)).min(4.0) };
        *a = aa * k;
        *b = bb * k;
    });
    ulos
}

/// Sävyn kierto (astetta), kylläisyys- ja kirkkauskerroin Labissa.
pub fn hsv_saato(l: &Taso, ab: &Kentta, h: f32, s: f32, v: f32) -> (Taso, Kentta) {
    let mut ab2 = ab.clone();
    if h != 0.0 || s != 1.0 {
        let (sn, cs) = h.to_radians().sin_cos();
        ab2.x.data.par_iter_mut().zip(ab2.y.data.par_iter_mut()).for_each(|(a, b)| {
            let (x, y) = (*a, *b);
            *a = (x * cs - y * sn) * s;
            *b = (x * sn + y * cs) * s;
        });
    }
    let mut l2 = l.clone();
    if v != 1.0 {
        l2.data.par_iter_mut().for_each(|x| *x = (*x * v).clamp(0.0, 100.0));
    }
    (l2, ab2)
}

// --------------------------------------------------------------------------
// Referenssin tilasto ja siirto (kirkkauskaistoittain)
// --------------------------------------------------------------------------
const KAISTAT: [f32; 5] = [15.0, 35.0, 55.0, 75.0, 92.0];
const KAISTA_LEVEYS: f32 = 12.0;

#[inline]
fn kaistapainot(l: f32) -> [f32; 5] {
    let mut w = [0.0; 5];
    let mut s = 0.0;
    for k in 0..5 {
        w[k] = (-0.5 * ((l - KAISTAT[k]) / KAISTA_LEVEYS).powi(2)).exp();
        s += w[k];
    }
    let s = s.max(1e-6);
    w.iter_mut().for_each(|x| *x /= s);
    w
}

#[derive(Clone, Debug)]
pub struct Tilasto {
    pub ka: [[f32; 2]; 5],
    pub haj: [[f32; 2]; 5],
}

pub fn ab_tilasto(l: &Taso, ab: &Kentta) -> Tilasto {
    let n = l.data.len();
    let vari: Vec<bool> = (0..n).map(|i| ab.x.data[i].hypot(ab.y.data[i]) > 3.0).collect();
    let kaikki = vari.iter().filter(|&&v| v).count() < 50;
    let mut p = [0.0f64; 5];
    let mut s = [[0.0f64; 2]; 5];
    let mut s2 = [[0.0f64; 2]; 5];
    let (mut ya, mut yb, mut ya2, mut yb2, mut m) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    for i in 0..n {
        if !kaikki && !vari[i] {
            continue;
        }
        let (a, b) = (ab.x.data[i] as f64, ab.y.data[i] as f64);
        let w = kaistapainot(l.data[i]);
        for k in 0..5 {
            let wk = w[k] as f64;
            p[k] += wk;
            s[k][0] += wk * a;
            s[k][1] += wk * b;
            s2[k][0] += wk * a * a;
            s2[k][1] += wk * b * b;
        }
        ya += a;
        yb += b;
        ya2 += a * a;
        yb2 += b * b;
        m += 1;
    }
    let m = m.max(1) as f64;
    let yka = [ya / m, yb / m];
    let yhaj = [((ya2 / m - yka[0].powi(2)).max(0.0)).sqrt().max(1.0), ((yb2 / m - yka[1].powi(2)).max(0.0)).sqrt().max(1.0)];
    let mut t = Tilasto { ka: [[0.0; 2]; 5], haj: [[1.0; 2]; 5] };
    for k in 0..5 {
        let pk = p[k] + 1e-6;
        let luotto = (pk / (0.02 * m + 1e-6)).clamp(0.0, 1.0);
        for c in 0..2 {
            let ka = s[k][c] / pk;
            let haj = (s2[k][c] / pk - ka * ka).max(1.0).sqrt();
            t.ka[k][c] = (luotto * ka + (1.0 - luotto) * yka[c]) as f32;
            t.haj[k][c] = (luotto * haj + (1.0 - luotto) * yhaj[c]) as f32;
        }
    }
    t
}

pub fn siirra_tilastoon(l: &Taso, ab: &Kentta, lahde: &Tilasto, kohde: &Tilasto, voima: f32) -> Kentta {
    if voima <= 0.0 {
        return ab.clone();
    }
    let mut skaala = [[1.0f32; 2]; 5];
    for k in 0..5 {
        for c in 0..2 {
            skaala[k][c] = (kohde.haj[k][c] / lahde.haj[k][c]).clamp(0.4, 2.5);
        }
    }
    let mut ulos = ab.clone();
    ulos.x.data.par_iter_mut().zip(ulos.y.data.par_iter_mut()).enumerate().for_each(|(i, (a, b))| {
        let w = kaistapainot(l.data[i]);
        let (x, y) = (*a, *b);
        let (mut na, mut nb) = (0.0, 0.0);
        for k in 0..5 {
            na += w[k] * ((x - lahde.ka[k][0]) * skaala[k][0] + kohde.ka[k][0]);
            nb += w[k] * ((y - lahde.ka[k][1]) * skaala[k][1] + kohde.ka[k][1]);
        }
        *a = (1.0 - voima) * x + voima * na;
        *b = (1.0 - voima) * y + voima * nb;
    });
    ulos
}

// --------------------------------------------------------------------------
// Referenssikuva
// --------------------------------------------------------------------------
const LB: usize = 48;
const SB: usize = 12;

pub struct Referenssi {
    pub tilasto: Tilasto,
    l: Taso,
    ab: Kentta,
    taulu: OnceLock<Taulu>,
}

struct Taulu {
    ab: Vec<[f32; 2]>,
    l_ka: f32,
    l_haj: f32,
    l_min: f32,
    l_max: f32,
    haj_max: f32,
}

fn piirteet(l: &Taso) -> (Taso, Taso) {
    let ka = l.laatikko(5);
    let mut nelio = l.clone();
    nelio.data.iter_mut().for_each(|v| *v *= *v);
    let ka2 = nelio.laatikko(5);
    let haj = Taso {
        w: l.w,
        h: l.h,
        data: ka.data.iter().zip(&ka2.data).map(|(m, m2)| (m2 - m * m).max(0.0).sqrt()).collect(),
    };
    (ka, haj)
}

impl Referenssi {
    pub fn kuvasta(k: &Bgr) -> Self {
        let lab = bgr_lab(k);
        let tilasto = ab_tilasto(&lab.l, &lab.ab);
        Referenssi { tilasto, l: lab.l, ab: lab.ab, taulu: OnceLock::new() }
    }

    pub fn tiedostosta(polku: &str) -> anyhow::Result<Self> {
        let kuva = image::open(polku).map_err(|e| anyhow::anyhow!("Cannot read the reference image: {polku}: {e}"))?;
        let kuva = if kuva.width().max(kuva.height()) > 1024 { kuva.thumbnail(1024, 1024) } else { kuva };
        let rgb = kuva.to_rgb8();
        Ok(Self::kuvasta(&Bgr::rgb_kuvasta(rgb.width() as usize, rgb.height() as usize, rgb.as_raw())))
    }

    fn taulu(&self) -> &Taulu {
        self.taulu.get_or_init(|| {
            let (l, haj) = piirteet(&self.l);
            let n = l.data.len() as f32;
            let l_ka = l.data.iter().sum::<f32>() / n;
            let l_haj = (l.data.iter().map(|v| (v - l_ka).powi(2)).sum::<f32>() / n).sqrt() + 1e-3;
            let mut lajit = haj.data.clone();
            lajit.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let haj_max = lajit[((lajit.len() as f32 * 0.99) as usize).min(lajit.len() - 1)] + 1e-3;
            let l_min = l.data.iter().cloned().fold(f32::MAX, f32::min);
            let l_max = l.data.iter().cloned().fold(f32::MIN, f32::max);
            let mut summa = vec![[0.0f32; 2]; LB * SB];
            let mut maara = vec![0.0f32; LB * SB];
            for i in 0..l.data.len() {
                let li = (((l.data[i] - l_min) / (l_max - l_min + 1e-3)) * (LB - 1) as f32).round().clamp(0.0, (LB - 1) as f32) as usize;
                let si = ((haj.data[i] / haj_max) * (SB - 1) as f32).round().clamp(0.0, (SB - 1) as f32) as usize;
                summa[li * SB + si][0] += self.ab.x.data[i];
                summa[li * SB + si][1] += self.ab.y.data[i];
                maara[li * SB + si] += 1.0;
            }
            // tyhjät lokerot täytetään normalisoidulla sumennuksella
            for sigma in [1.0f32, 2.5, 6.0, 15.0] {
                let n_t = Taso { w: SB, h: LB, data: maara.clone() }.gauss(sigma);
                let sa = Taso { w: SB, h: LB, data: summa.iter().map(|v| v[0]).collect() }.gauss(sigma);
                let sb = Taso { w: SB, h: LB, data: summa.iter().map(|v| v[1]).collect() }.gauss(sigma);
                for i in 0..LB * SB {
                    if maara[i] < 0.5 {
                        summa[i] = [sa.data[i], sb.data[i]];
                        maara[i] = n_t.data[i];
                    }
                }
            }
            let ab = summa.iter().zip(&maara).map(|(s, &n)| [s[0] / n.max(1e-6), s[1] / n.max(1e-6)]).collect();
            Taulu { ab, l_ka, l_haj, l_min, l_max, haj_max }
        })
    }

    /// Värittää pelkän kirkkauskuvan referenssin perusteella (Welsh ym. 2002, ei neuroverkkoa).
    pub fn siirra_harmaaseen(&self, l: &Taso) -> Kentta {
        let t = self.taulu();
        let (lp, haj) = piirteet(l);
        let n = lp.data.len() as f32;
        let ka = lp.data.iter().sum::<f32>() / n;
        let sd = (lp.data.iter().map(|v| (v - ka).powi(2)).sum::<f32>() / n).sqrt() + 1e-3;
        let mut ab = Kentta::new(l.w, l.h);
        ab.x.data.par_iter_mut().zip(ab.y.data.par_iter_mut()).enumerate().for_each(|(i, (a, b))| {
            let lv = (lp.data[i] - ka) / sd * t.l_haj + t.l_ka;
            let li = (((lv - t.l_min) / (t.l_max - t.l_min + 1e-3)) * (LB - 1) as f32).round().clamp(0.0, (LB - 1) as f32) as usize;
            let si = ((haj.data[i] / t.haj_max) * (SB - 1) as f32).round().clamp(0.0, (SB - 1) as f32) as usize;
            let v = t.ab[li * SB + si];
            *a = v[0];
            *b = v[1];
        });
        ab.gauss(1.5)
    }
}
