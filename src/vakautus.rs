//! Vilkkumisen poisto: kohtausrajat, optiseen virtaukseen perustuva aikatasaus (keskitetty ikkuna)
//! ja värien lukitus (iho / taivas / kasvillisuus).

use std::collections::VecDeque;

use rayon::prelude::*;

use crate::kuva::{Kentta, Taso};
use crate::virtaus::{ketjuta, kohdista, kohdista_kentta, virtaus};

pub const VIRTAUS_LEVEYS: usize = 480;

/// Kirkkaus (0..100) -> pieni harmaakuva (0..255) virtausta ja leikkauksia varten.
pub fn pieni_harmaa(l: &Taso) -> Taso {
    let s = (VIRTAUS_LEVEYS as f32 / l.w as f32).min(1.0);
    let mut g = l.clone();
    g.data.iter_mut().for_each(|v| *v = (*v * 2.55).clamp(0.0, 255.0));
    if s < 1.0 {
        g = g.koon(((l.w as f32 * s) as usize / 2 * 2).max(2), ((l.h as f32 * s) as usize / 2 * 2).max(2));
    }
    g
}

pub struct Leikkaukset {
    raja: f32,
    edellinen: Option<([f32; 32], Taso)>,
}

impl Leikkaukset {
    pub fn new(raja: f32) -> Self {
        Leikkaukset { raja, edellinen: None }
    }

    /// g: harmaakuva 0..255.
    pub fn onko(&mut self, g: &Taso) -> bool {
        let mut hist = [0.0f32; 32];
        for &v in &g.data {
            hist[((v.clamp(0.0, 255.0)) as usize / 8).min(31)] += 1.0;
        }
        let n = g.data.len().max(1) as f32;
        hist.iter_mut().for_each(|x| *x /= n);
        let mut tulos = false;
        if let Some((eh, eg)) = &self.edellinen {
            let ero_hist = 0.5 * hist.iter().zip(eh).map(|(a, b)| (a - b).abs()).sum::<f32>();
            let ero_pix = if eg.data.len() == g.data.len() {
                g.data.iter().zip(&eg.data).map(|(a, b)| (a.round() - b.round()).abs()).sum::<f32>() / n / 255.0
            } else {
                1.0
            };
            tulos = ero_hist > self.raja || (ero_hist > self.raja * 0.5 && ero_pix > 0.18);
        }
        self.edellinen = Some((hist, g.clone()));
        tulos
    }
}

struct Jasen<D> {
    data: Option<D>,
    ab: Kentta,
    ab_p: Kentta,
    g: Taso,
    kohtaus: usize,
    v_ed: Option<Kentta>,
    v_seur: Option<Kentta>,
}

/// Keskitetty ikkuna: kehyksen a/b on painotettu keskiarvo naapureista, jotka on siirretty
/// optisella virtauksella samaan kohtaan. Paino pienenee, jos kohdistus ei onnistu. Ei ylitä kohtausrajoja.
/// Virtaus lasketaan vain peräkkäisille kuville (molempiin suuntiin); kauemmas ketjutetaan.
pub struct Aikatasaus<D> {
    r: usize,
    virtaus: bool,
    voima: f32,
    paino_aika: Vec<f32>,
    jono: VecDeque<Jasen<D>>,
    seuraava: usize,
    kohtaus: usize,
}

impl<D> Aikatasaus<D> {
    pub fn new(ikkuna: usize, virtaus: bool, voima: f32) -> Self {
        let r = ikkuna / 2;
        let s = (r.max(1) as f32) * 0.75;
        Aikatasaus {
            r,
            virtaus,
            voima,
            paino_aika: (0..=r).map(|d| (-0.5 * (d as f32 / s).powi(2)).exp()).collect(),
            jono: VecDeque::new(),
            seuraava: 0,
            kohtaus: 0,
        }
    }

    /// `v_ed`: valmiiksi laskettu virtaus tästä kuvasta edelliseen (värimuisti laskee sen jo), muuten lasketaan.
    pub fn lisaa(&mut self, data: D, ab: Kentta, g: Taso, leikkaus: bool, v_ed: Option<Kentta>) -> Vec<(D, Kentta)> {
        if leikkaus {
            self.kohtaus += 1;
        }
        let ab_p = ab.koon(g.w, g.h);
        let mut uusi = Jasen { data: Some(data), ab, ab_p, g, kohtaus: self.kohtaus, v_ed: None, v_seur: None };
        if self.virtaus && self.r > 0 {
            if let Some(ed) = self.jono.back_mut() {
                if ed.kohtaus == self.kohtaus {
                    let (a, b) = match v_ed {
                        Some(b) => (virtaus(&ed.g, &uusi.g), b),
                        None => rayon::join(|| virtaus(&ed.g, &uusi.g), || virtaus(&uusi.g, &ed.g)),
                    };
                    ed.v_seur = Some(a);
                    uusi.v_ed = Some(b);
                }
            }
        }
        self.jono.push_back(uusi);
        let mut ulos = Vec::new();
        while self.seuraava + self.r < self.jono.len() {
            ulos.push(self.laske(self.seuraava));
            self.seuraava += 1;
            while self.seuraava > self.r {
                self.jono.pop_front();
                self.seuraava -= 1;
            }
        }
        ulos
    }

    pub fn tyhjenna(&mut self) -> Vec<(D, Kentta)> {
        let mut ulos = Vec::new();
        while self.seuraava < self.jono.len() {
            ulos.push(self.laske(self.seuraava));
            self.seuraava += 1;
        }
        self.jono.clear();
        self.seuraava = 0;
        ulos
    }

    fn laske(&mut self, i: usize) -> (D, Kentta) {
        let data = self.jono[i].data.take().expect("kehys laskettu kahdesti");
        if self.r == 0 || self.voima <= 0.0 {
            return (data, self.jono[i].ab.clone());
        }
        let j0 = &self.jono[i];
        let (gw, gh) = (j0.g.w, j0.g.h);
        let p0 = self.paino_aika[0];
        let mut sx: Vec<f32> = j0.ab_p.x.data.iter().map(|v| v * p0).collect();
        let mut sy: Vec<f32> = j0.ab_p.y.data.iter().map(|v| v * p0).collect();
        let mut sp = vec![p0; gw * gh];
        for suunta in [1i64, -1] {
            let mut v: Option<Kentta> = None;
            for d in 1..=self.r {
                let j = i as i64 + suunta * d as i64;
                if j < 0 || j as usize >= self.jono.len() || self.jono[j as usize].kohtaus != j0.kohtaus {
                    break;
                }
                let jj = &self.jono[j as usize];
                let (ab_j, g_j) = if self.virtaus {
                    let edellinen = &self.jono[(j - suunta) as usize];
                    let askel = if suunta > 0 { &edellinen.v_seur } else { &edellinen.v_ed };
                    let Some(askel) = askel else { break };
                    let uusi = match &v {
                        None => askel.clone(),
                        Some(vv) => ketjuta(vv, askel),
                    };
                    let k = (kohdista_kentta(&jj.ab_p, &uusi), kohdista(&jj.g, &uusi));
                    v = Some(uusi);
                    k
                } else {
                    (jj.ab_p.clone(), jj.g.clone())
                };
                let ero = Taso { w: gw, h: gh, data: j0.g.data.iter().zip(&g_j.data).map(|(a, b)| (a - b).abs()).collect() }.gauss(1.5);
                let pa = self.paino_aika[d];
                sx.par_iter_mut()
                    .zip(sy.par_iter_mut())
                    .zip(sp.par_iter_mut())
                    .enumerate()
                    .for_each(|(k, ((x, y), p))| {
                        let e = ero.data[k] / 6.0;
                        let l = (-0.5 * e * e).exp() * pa;
                        *x += ab_j.x.data[k] * l;
                        *y += ab_j.y.data[k] * l;
                        *p += l;
                    });
            }
        }
        let korjaus = Kentta {
            x: Taso { w: gw, h: gh, data: sx.iter().zip(&sp).zip(&j0.ab_p.x.data).map(|((s, p), a)| s / p - a).collect() },
            y: Taso { w: gw, h: gh, data: sy.iter().zip(&sp).zip(&j0.ab_p.y.data).map(|((s, p), a)| s / p - a).collect() },
        };
        let korjaus = korjaus.koon(j0.ab.w(), j0.ab.h());
        let mut ab = j0.ab.clone();
        let voima = self.voima;
        ab.x.data.par_iter_mut().zip(&korjaus.x.data).for_each(|(a, k)| *a += voima * k);
        ab.y.data.par_iter_mut().zip(&korjaus.y.data).for_each(|(a, k)| *a += voima * k);
        (data, ab)
    }
}

// --------------------------------------------------------------------------
// Värimuisti
// --------------------------------------------------------------------------
/// Kohtauksen värimuisti: väri on juokseva keskiarvo liikeradan varrella. Edellisen kuvan muisti ja
/// sen kuvamäärä kuljetetaan optisella virtauksella uuden kuvan kohdalle, ja mallin uusi väri lisätään
/// keskiarvoon samalla painolla kuin aiemmat (enintään `n_max` = voima/(1-voima) viimeistä kuvaa).
/// Näin esine pitää koko kohtauksen ajan värin, jota malli on sille enimmäkseen ehdottanut – ei ensimmäisen
/// kuvan arvausta eikä yksittäisen kuvan horjahdusta. Kohdissa, joissa kuljetettu kuva ei täsmää (esiin
/// tuleva tausta, nopea liike, kuvan reuna), keskiarvo alkaa alusta mallin väristä. Nollautuu leikkauksessa.
/// Korjaus lasketaan pienessä koossa ja lisätään täysikokoiseen väriin, joten mallin yksityiskohdat säilyvät.
pub struct Varimuisti {
    voima: f32,
    edellinen: Option<(Taso, Kentta, Taso)>, // pieni harmaa, muistin a/b ja kuvamäärä samassa koossa
    /// kohtausmuistin antama lähtöarvo seuraavan kohtauksen alkuun (a/b ja paino 0..1)
    siemen: Option<(Kentta, Taso)>,
}

impl Varimuisti {
    pub fn new(voima: f32) -> Self {
        Varimuisti { voima: voima.clamp(0.0, 0.98), edellinen: None, siemen: None }
    }

    /// Kohtauksen alkuun aiemman kohtauksen värit (painon verran): ne vastaavat useaa jo nähtyä kuvaa.
    pub fn aseta_siemen(&mut self, ab: Kentta, paino: Taso) {
        self.siemen = Some((ab, paino));
    }

    /// Muistin nykyinen väri (pienessä koossa), kohtausmuistin ankkuria varten.
    pub fn muisti_ab(&self) -> Option<&Kentta> {
        self.edellinen.as_ref().map(|e| &e.1)
    }

    pub fn kaytossa(&self) -> bool {
        self.voima > 0.0
    }

    /// Virtaus kuvasta `g` edelliseen (I_g(x) ≈ I_ed(x + v)); None kohtauksen alussa.
    pub fn virtaus_edelliseen(&self, g: &Taso, leikkaus: bool) -> Option<Kentta> {
        match &self.edellinen {
            Some((ge, _, _)) if !leikkaus && ge.w == g.w && ge.h == g.h => Some(virtaus(g, ge)),
            _ => None,
        }
    }

    pub fn kasittele(&mut self, g: &Taso, ab: Kentta, v_ed: Option<&Kentta>) -> Kentta {
        if !self.kaytossa() {
            return ab;
        }
        let (gw, gh) = (g.w, g.h);
        let ab_p = ab.koon(gw, gh);
        let n_max = self.voima / (1.0 - self.voima);
        let (ulos_p, maara) = match (&self.edellinen, v_ed) {
            (Some((ge, me, ne)), Some(v)) => {
                let ((mk, nk), gk) = rayon::join(|| (kohdista_kentta(me, v), kohdista(ne, v)), || kohdista(ge, v));
                let ero = Taso { w: gw, h: gh, data: g.data.iter().zip(&gk.data).map(|(a, b)| (a - b).abs()).collect() }.gauss(1.5);
                let mut u = ab_p.clone();
                let mut maara = Taso::new(gw, gh);
                u.x.data.par_chunks_mut(gw).zip(u.y.data.par_chunks_mut(gw)).zip(maara.data.par_chunks_mut(gw)).enumerate()
                    .for_each(|(y, ((rx, ry), rn))| {
                        for x in 0..gw {
                            let i = y * gw + x;
                            let (sx, sy) = (x as f32 + v.x.data[i], y as f32 + v.y.data[i]);
                            // kuvan ulkopuolelta tuleva kohta on uutta sisältöä
                            let n = if sx < 0.0 || sy < 0.0 || sx > (gw - 1) as f32 || sy > (gh - 1) as f32 {
                                0.0
                            } else {
                                let e = ero.data[i] / 6.0;
                                (nk.data[i] * (-0.5 * e * e).exp()).min(n_max)
                            };
                            let t = n / (n + 1.0);
                            rx[x] = t * mk.x.data[i] + (1.0 - t) * rx[x];
                            ry[x] = t * mk.y.data[i] + (1.0 - t) * ry[x];
                            rn[x] = n + 1.0;
                        }
                    });
                (u, maara)
            }
            _ => match self.siemen.take().filter(|(s, _)| s.w() == gw && s.h() == gh) {
                Some((s, p)) => {
                    let n0 = n_max.min(30.0);
                    let mut u = ab_p.clone();
                    u.x.data.iter_mut().zip(&s.x.data).zip(&p.data).for_each(|((a, b), t)| *a = t * b + (1.0 - t) * *a);
                    u.y.data.iter_mut().zip(&s.y.data).zip(&p.data).for_each(|((a, b), t)| *a = t * b + (1.0 - t) * *a);
                    (u, Taso { w: gw, h: gh, data: p.data.iter().map(|t| 1.0 + n0 * t).collect() })
                }
                None => (ab_p.clone(), Taso { w: gw, h: gh, data: vec![1.0; gw * gh] }),
            },
        };
        let korjaus = Kentta {
            x: Taso { w: gw, h: gh, data: ulos_p.x.data.iter().zip(&ab_p.x.data).map(|(u, a)| u - a).collect() },
            y: Taso { w: gw, h: gh, data: ulos_p.y.data.iter().zip(&ab_p.y.data).map(|(u, a)| u - a).collect() },
        }
        .koon(ab.w(), ab.h());
        self.edellinen = Some((g.clone(), ulos_p, maara));
        let mut ab = ab;
        ab.x.data.par_iter_mut().zip(&korjaus.x.data).for_each(|(a, k)| *a += k);
        ab.y.data.par_iter_mut().zip(&korjaus.y.data).for_each(|(a, k)| *a += k);
        ab
    }

    /// Kohtauksen vaihtuessa muisti tyhjennetään.
    pub fn nollaa(&mut self) {
        self.edellinen = None;
        self.siemen = None;
    }
}

/// Värin välkyntä kahden peräkkäisen kuvan välillä: edellisen a/b kuljetetaan virtauksella nykyisen
/// kohdalle ja verrataan niissä kohdissa, joissa kuva täsmää (Lab-yksikköä, 0 = täysin vakaa).
pub fn valkynta(g: &Taso, ab: &Kentta, g_ed: &Taso, ab_ed: &Kentta) -> Option<f32> {
    let v = virtaus(g, g_ed);
    let gk = kohdista(g_ed, &v);
    let abk = kohdista_kentta(ab_ed, &v);
    let ero = Taso { w: g.w, h: g.h, data: g.data.iter().zip(&gk.data).map(|(a, b)| (a - b).abs()).collect() }.gauss(1.0);
    let (mut s, mut n) = (0.0f64, 0usize);
    for i in 0..g.data.len() {
        if ero.data[i] < 4.0 {
            s += ((ab.x.data[i] - abk.x.data[i]).hypot(ab.y.data[i] - abk.y.data[i])) as f64;
            n += 1;
        }
    }
    if n * 4 < g.data.len() { None } else { Some((s / n as f64) as f32) }
}

// --------------------------------------------------------------------------
// Värien lukitus
// --------------------------------------------------------------------------
/// (sävykulma-alue astetta, kroma-alue, L-alue, vain yläosa)
fn luokan_rajat(nimi: &str) -> ((f32, f32), (f32, f32), (f32, f32), bool) {
    match nimi {
        "iho" => ((20.0, 80.0), (5.0, 55.0), (20.0, 95.0), false),
        "taivas" => ((195.0, 290.0), (4.0, 60.0), (45.0, 100.0), true),
        _ => ((95.0, 175.0), (5.0, 70.0), (10.0, 90.0), false),
    }
}

#[inline]
fn pehmea(x: f32, ala: f32, yla: f32, reuna: f32) -> f32 {
    ((x - ala).min(yla - x) / reuna + 0.5).clamp(0.0, 1.0)
}

pub fn luokkamaski(nimi: &str, l: &Taso, ab: &Kentta) -> Taso {
    let ((s0, s1), (c0, c1), (l0, l1), yla) = luokan_rajat(nimi);
    let (w, h) = (l.w, l.h);
    let mut m = Taso::new(w, h);
    m.data.par_chunks_mut(w).enumerate().for_each(|(y, r)| {
        let yy = 1.0 - y as f32 / (h.max(2) - 1) as f32;
        let ym = if yla { ((yy - 0.35) / 0.25).clamp(0.0, 1.0) } else { 1.0 };
        for x in 0..w {
            let i = y * w + x;
            let (a, b) = (ab.x.data[i], ab.y.data[i]);
            let savy = (b.atan2(a).to_degrees() + 360.0) % 360.0;
            let c = a.hypot(b);
            r[x] = pehmea(savy, s0, s1, 12.0) * pehmea(c, c0, c1, 4.0) * pehmea(l.data[i], l0, l1, 8.0) * ym;
        }
    });
    m.gauss(2.0)
}

/// Pitää valittujen alueiden keskisävyn vakiona kohtauksen sisällä: alue siirretään kokonaisuutena
/// kohtauksen kertyneeseen keskiarvoon (tekstuuri säilyy). Nollautuu kohtauksen vaihtuessa.
pub struct Varilukitus {
    voima: f32,
    luokat: Vec<String>,
    ankkuri: Vec<Option<[f32; 2]>>,
    kertyma: Vec<f32>,
}

impl Varilukitus {
    pub fn new(voima: f32, luokat: &[String]) -> Self {
        Varilukitus { voima, luokat: luokat.to_vec(), ankkuri: vec![None; luokat.len()], kertyma: vec![0.0; luokat.len()] }
    }

    pub fn kasittele(&mut self, l: &Taso, ab: Kentta, leikkaus: bool) -> Kentta {
        if self.voima <= 0.0 || self.luokat.is_empty() {
            return ab;
        }
        if leikkaus {
            self.ankkuri.iter_mut().for_each(|a| *a = None);
            self.kertyma.iter_mut().for_each(|k| *k = 0.0);
        }
        let (w, h) = (ab.w(), ab.h());
        let s = (320.0 / w as f32).min(1.0);
        let (pw, ph) = (((w as f32 * s) as usize).max(2), ((h as f32 * s) as usize).max(2));
        let ab_p = ab.koon(pw, ph);
        let l_p = l.koon(pw, ph);
        let mut siirto = Kentta::new(pw, ph);
        let koko = (pw * ph) as f32;
        for (k, nimi) in self.luokat.clone().iter().enumerate() {
            let m = luokkamaski(nimi, &l_p, &ab_p);
            let ala: f32 = m.data.iter().sum();
            if ala < 0.002 * koko {
                continue;
            }
            let ka = [
                m.data.iter().zip(&ab_p.x.data).map(|(m, a)| m * a).sum::<f32>() / ala,
                m.data.iter().zip(&ab_p.y.data).map(|(m, a)| m * a).sum::<f32>() / ala,
            ];
            let osuus = ala / koko;
            let n = (self.kertyma[k] + osuus).min(50.0 * osuus);
            let a = osuus / n.max(1e-6);
            self.kertyma[k] = n;
            let ank = match self.ankkuri[k] {
                None => ka,
                Some(v) => [(1.0 - a) * v[0] + a * ka[0], (1.0 - a) * v[1] + a * ka[1]],
            };
            self.ankkuri[k] = Some(ank);
            let (dx, dy) = (ank[0] - ka[0], ank[1] - ka[1]);
            for (i, mv) in m.data.iter().enumerate() {
                siirto.x.data[i] += self.voima * mv * dx;
                siirto.y.data[i] += self.voima * mv * dy;
            }
        }
        let siirto = siirto.koon(w, h);
        let mut ulos = ab;
        ulos.x.data.par_iter_mut().zip(&siirto.x.data).for_each(|(a, d)| *a += d);
        ulos.y.data.par_iter_mut().zip(&siirto.y.data).for_each(|(a, d)| *a += d);
        ulos
    }
}
