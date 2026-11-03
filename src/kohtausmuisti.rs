//! Kohtausmuisti: sama kohde saa saman värin leikkausten yli (vastakuvat, palaavat kuvakulmat).
//! Jokaisesta kohtauksesta tallennetaan ankkuri: DINOv2-piirteet ja lopullinen väri 14 px:n ruuduittain.
//! Uuden kohtauksen alussa jokaiselle ruudulle haetaan paras vastine aiemmista saman tilan kohtauksista
//! (koko kuvan piirteet lähellä toisiaan). Löytyneet värit annetaan värimuistille lähtöarvoksi, joten
//! malli voi vielä korjata niitä kohtauksen kuluessa, mutta yksittäinen kuvakulma ei vaihda väriä.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rayon::prelude::*;

use crate::kuva::{Kentta, Taso};
use crate::piirteet::{Dino, Piirteet, pistetulo};

/// Montako aiempaa kohtausta muistetaan (noin muutaman minuutin jakso).
const ANKKUREITA: usize = 60;
/// Koko kuvan samankaltaisuus, josta alkaen kohtaus katsotaan samaksi tilaksi.
const SAMA_TILA: f32 = 0.55;
/// Ruutujen samankaltaisuus: alle ALA ei siirretä, yli YLA täysi paino.
const ALA: f32 = 0.62;
const YLA: f32 = 0.78;
/// Ankkuri päivitetään kohtauksen kuvassa 6 ja sitten 25 kuvan välein (värimuisti on silloin vakiintunut).
const ENSIMMAINEN: usize = 6;
const VALI: usize = 25;

struct Ankkuri {
    f: Piirteet,
    ab: Vec<[f32; 2]>,
}

pub struct Kohtausmuisti {
    dino: Arc<Mutex<Dino>>,
    ankkurit: VecDeque<Ankkuri>,
    nykyinen: Option<Ankkuri>,
    kuvia: usize,
    pub kohtauksia: usize,
    pub siirrettyja: usize,
}

/// Kenttä tai taso keskiarvoina ruuduittain (gw x gh).
fn ruuduittain(t: &Taso, gw: usize, gh: usize) -> Vec<f32> {
    t.koon(gw, gh).data
}

impl Kohtausmuisti {
    /// DINOv2 ajetaan suorittimella: sitä tarvitaan harvoin (kohtauksen alussa ja 25 kuvan välein), eikä
    /// toinen DirectML-istunto saa kilpailla samanaikaisesti väritysmallin kanssa näytönohjaimesta.
    pub fn new() -> Result<Kohtausmuisti> {
        Ok(Kohtausmuisti { dino: crate::piirteet::hae("cpu")?, ankkurit: VecDeque::new(), nykyinen: None, kuvia: 0, kohtauksia: 0, siirrettyja: 0 })
    }

    /// Uusi kohtaus alkaa kuvasta `g` (harmaa 0..255). Palauttaa värimuistin lähtöarvon (a/b ja paino 0..1
    /// samassa koossa kuin `g`), jos kohtaus vastaa jotakin aiempaa.
    pub fn kohtauksen_alku(&mut self, g: &Taso) -> Result<Option<(Kentta, Taso)>> {
        if let Some(a) = self.nykyinen.take() {
            self.ankkurit.push_back(a);
            if self.ankkurit.len() > ANKKUREITA {
                self.ankkurit.pop_front();
            }
        }
        self.kuvia = 0;
        self.kohtauksia += 1;
        if self.ankkurit.is_empty() {
            return Ok(None);
        }
        let f = self.dino.lock().unwrap().piirteet(g)?;
        // saman tilan kohtaukset, uusimmat ensin, enintään 6
        let mut ehdokkaat: Vec<(f32, &Ankkuri)> = self.ankkurit.iter().rev()
            .map(|a| (pistetulo(&f.cls, &a.f.cls), a))
            .filter(|(s, _)| *s >= SAMA_TILA)
            .take(6)
            .collect();
        if ehdokkaat.is_empty() {
            return Ok(None);
        }
        ehdokkaat.sort_by(|a, b| b.0.total_cmp(&a.0));
        // paras vastine jokaiselle ruudulle
        let osumat: Vec<(f32, [f32; 2])> = (0..f.n()).into_par_iter().map(|i| {
            let r = f.ruutu(i);
            let mut paras = (-1.0f32, [0.0f32; 2]);
            for (_, a) in &ehdokkaat {
                for j in 0..a.f.n() {
                    let s = pistetulo(r, a.f.ruutu(j));
                    if s > paras.0 {
                        paras = (s, a.ab[j]);
                    }
                }
            }
            paras
        }).collect();
        let painot: Vec<f32> = osumat.iter().map(|(s, _)| ((s - ALA) / (YLA - ALA)).clamp(0.0, 1.0)).collect();
        let kattavuus = painot.iter().filter(|&&p| p > 0.5).count() as f32 / painot.len() as f32;
        if kattavuus < 0.2 {
            return Ok(None);
        }
        self.siirrettyja += 1;
        // ruutujen väri pikseleille: lähimmät ruudut, painotettuna kirkkauden samankaltaisuudella (reunat säilyvät)
        let (gw, gh) = (f.gw, f.gh);
        let valo = ruuduittain(g, gw, gh);
        let (w, h) = (g.w, g.h);
        let mut kohde = Kentta::new(w, h);
        let mut paino = Taso::new(w, h);
        kohde.x.data.par_chunks_mut(w).zip(kohde.y.data.par_chunks_mut(w)).zip(paino.data.par_chunks_mut(w)).enumerate()
            .for_each(|(y, ((rx, ry), rp))| {
                let fy = (y as f32 + 0.5) * gh as f32 / h as f32 - 0.5;
                for x in 0..w {
                    let fx = (x as f32 + 0.5) * gw as f32 / w as f32 - 0.5;
                    let v = g.data[y * w + x];
                    let (mut sa, mut sb, mut sp, mut sk) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                    let (cx, cy) = (fx.round() as i64, fy.round() as i64);
                    for j in cy - 1..=cy + 1 {
                        for i in cx - 1..=cx + 1 {
                            if i < 0 || j < 0 || i >= gw as i64 || j >= gh as i64 {
                                continue;
                            }
                            let k = j as usize * gw + i as usize;
                            let d2 = (i as f32 - fx).powi(2) + (j as f32 - fy).powi(2);
                            let e = (v - valo[k]) / 18.0;
                            let s = (-d2 / (2.0 * 0.6 * 0.6)).exp() * (-0.5 * e * e).exp();
                            let p = s * painot[k];
                            sa += p * osumat[k].1[0];
                            sb += p * osumat[k].1[1];
                            sp += p;
                            sk += s;
                        }
                    }
                    if sp > 1e-6 {
                        rx[x] = sa / sp;
                        ry[x] = sb / sp;
                        rp[x] = sp / sk.max(1e-6);
                    }
                }
            });
        Ok(Some((kohde, paino.gauss(1.0))))
    }

    /// Kohtauksen kuluessa: ankkuri päivitetään värimuistin vakiintuneesta väristä (`ab` samaa kokoa kuin `g`).
    pub fn paivita(&mut self, g: &Taso, ab: Option<&Kentta>) -> Result<()> {
        self.kuvia += 1;
        let Some(ab) = ab else { return Ok(()) };
        if self.kuvia != ENSIMMAINEN && (self.kuvia < ENSIMMAINEN || (self.kuvia - ENSIMMAINEN) % VALI != 0) {
            return Ok(());
        }
        let f = self.dino.lock().unwrap().piirteet(g)?;
        let a = ruuduittain(&ab.x, f.gw, f.gh);
        let b = ruuduittain(&ab.y, f.gw, f.gh);
        let ab = a.into_iter().zip(b).map(|(a, b)| [a, b]).collect();
        self.nykyinen = Some(Ankkuri { f, ab });
        Ok(())
    }

    pub fn yhteenveto(&self) -> String {
        format!("{} / {} scenes took their colours from an earlier scene in the same place", self.siirrettyja, self.kohtauksia)
    }
}
