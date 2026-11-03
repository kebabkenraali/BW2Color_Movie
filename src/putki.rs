//! Väritysputki: väritys → tarkistus → korjaus yhdellä ajolla.
//!
//! 1. Väritys: jokainen kuva väritetään; kuvakohtainen laadunvalvonta täyttää värittämättömät läikät ja
//!    harmaat ruudut ja vertaa värejä muutamaan edelliseen saman kohtauksen kuvaan (tasalaatuisuus).
//! 2. Tarkistus: valmis välitulos käydään läpi kuva kuvalta (harmaat ruudut, läikät, värihypyt).
//! 3. Korjaus: jos ongelmia löytyi, välitulos korjataan mustavalko-originaalia vasten (kirkkaus
//!    alkuperäisestä, täyttö, aikatasaus). Lopputulos tarkistetaan uudelleen.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::kuva::{Bgr, Kentta, Taso};
use crate::mallit::{self, Malli};
use crate::tuotanto::{Asetukset, Tuottaja, laikut};
use crate::vakautus::{Leikkaukset, pieni_harmaa};
use crate::vari::{Tilasto, ab_tilasto, bgr_lab, kroma, siirra_tilastoon};
use crate::tunnistin::Tunnistin;
use crate::video::{Lukija, Tiedot, aikateksti};
use crate::{edistys, loki};

/// Montako edellistä saman kohtauksen kuvaa toimii vertailukohtana.
pub const VERTAILUKUVIA: usize = 6;
/// Väritilaston ero (Lab-yksikköä), jonka ylittävä muutos edellisiin kuviin tasataan.
const HYPPYRAJA: f32 = 5.0;

/// Ruudun keskikroma keskisävyissä (hyvin tummat ja kirkkaat pikselit eivät kerro värityksestä).
pub fn ruudun_kroma(lw: &Taso, kr: &Taso) -> f32 {
    let (mut s, mut m) = (0.0f64, 0usize);
    for (j, &l) in lw.data.iter().enumerate() {
        if l > 12.0 && l < 95.0 {
            s += kr.data[j] as f64;
            m += 1;
        }
    }
    if m > 0 { (s / m as f64) as f32 } else { 0.0 }
}

fn keskivalo(lw: &Taso) -> f32 {
    lw.data.iter().sum::<f32>() / lw.data.len().max(1) as f32
}

/// Kahden väritilaston ero: keskiarvojen ja hajontojen keskimääräinen poikkeama Lab-yksikköinä.
pub fn tilastoero(a: &Tilasto, b: &Tilasto) -> f32 {
    let mut s = 0.0;
    for k in 0..5 {
        for c in 0..2 {
            s += (a.ka[k][c] - b.ka[k][c]).abs() + 0.5 * (a.haj[k][c] - b.haj[k][c]).abs();
        }
    }
    s / 10.0
}

fn keskiarvo(h: &VecDeque<Tilasto>) -> Tilasto {
    let mut t = Tilasto { ka: [[0.0; 2]; 5], haj: [[0.0; 2]; 5] };
    let n = h.len().max(1) as f32;
    for x in h {
        for k in 0..5 {
            for c in 0..2 {
                t.ka[k][c] += x.ka[k][c] / n;
                t.haj[k][c] += x.haj[k][c] / n;
            }
        }
    }
    t
}

/// Kuvakohtainen laadunvalvonta värityksen ja korjauksen aikana.
pub struct Laadunvalvonta {
    /// täyttömalli ladataan vasta, kun sitä ensimmäisen kerran tarvitaan (säästää näytönohjaimen muistia)
    tayttomalli: Option<String>,
    tayttaja: Option<Arc<Mutex<Malli>>>,
    laite: String,
    harmaa_raja: f32,
    historia: VecDeque<Tilasto>,
    /// värittämättömien kohtien tunnistin (oma malli), jos asennettu
    tunnistin: Option<Tunnistin>,
    pub harmaita: usize,
    pub laikkuja: usize,
    pub tunnistettuja: usize,
    pub tasattuja: usize,
}

impl Laadunvalvonta {
    /// `paamalli` = värityksen oma malli: täyttöön käytetään mieluiten eri mallia, koska sama malli
    /// jättää samat kohdat harmaiksi.
    pub fn new(tayta_malli: &str, paamalli: Option<&str>, laite: &str, harmaa_raja: f32) -> Result<Laadunvalvonta> {
        let mut ehdokkaat = vec![tayta_malli.to_string()];
        for m in ["ddcolor_suomi_technicolor", "ddcolor_suomi_eastmancolor", "ddcolor_suomi"] {
            ehdokkaat.push(m.to_string());
        }
        let valittu = ehdokkaat.iter().filter(|m| mallit::on_ladattu(m)).find(|m| Some(m.as_str()) != paamalli)
            .or_else(|| ehdokkaat.iter().find(|m| mallit::on_ladattu(m)));
        match valittu {
            Some(m) => loki(&format!("Quality control: grey areas are filled with model {m}, compared with {VERTAILUKUVIA} previous frames.")),
            None => loki("Quality control: the fill model is not installed – grey areas are not filled."),
        }
        let tunnistin = match crate::tunnistin::polku().map(|_| Tunnistin::lataa(laite)) {
            Some(Ok(t)) => {
                loki("Quality control: uncoloured-area detector in use (faces, people, objects).");
                Some(t)
            }
            Some(Err(e)) => {
                loki(&format!("Quality control: could not load the detector: {e:#}"));
                None
            }
            None => None,
        };
        Ok(Laadunvalvonta {
            tayttomalli: valittu.cloned(),
            tayttaja: None,
            laite: laite.to_string(),
            harmaa_raja,
            historia: VecDeque::new(),
            tunnistin,
            harmaita: 0,
            laikkuja: 0,
            tunnistettuja: 0,
            tasattuja: 0,
        })
    }

    fn varita(&mut self, mv: &Bgr) -> Result<Option<Kentta>> {
        if self.tayttaja.is_none() {
            match &self.tayttomalli {
                Some(m) => self.tayttaja = Some(mallit::hae(m, &self.laite)?),
                None => return Ok(None),
            }
        }
        let t = self.tayttaja.as_ref().unwrap();
        Ok(Some(t.lock().unwrap().ab(&[mv])?.remove(0)))
    }

    /// `mv` = mustavalkokuva työkoossa, `lw` = sen kirkkaus, `ab` = väritys. Palauttaa korjatun värityksen.
    pub fn kasittele(&mut self, mv: &Bgr, lw: &Taso, mut ab: Kentta, leikkaus: bool) -> Result<Kentta> {
        if leikkaus {
            self.historia.clear();
        }
        let kr = kroma(&ab);
        let rk = ruudun_kroma(lw, &kr);
        let vertailu = (!self.historia.is_empty()).then(|| keskiarvo(&self.historia));
        if rk < self.harmaa_raja {
            // koko ruutu jäi harmaaksi: väritetään uudelleen ja sovitetaan kohtauksen edellisiin kuviin
            if let Some(uusi) = self.varita(mv)? {
                let ut = ab_tilasto(lw, &uusi);
                ab = match &vertailu {
                    Some(v) => siirra_tilastoon(lw, &uusi, &ut, v, 1.0),
                    None => uusi,
                };
                if ruudun_kroma(lw, &kroma(&ab)) >= self.harmaa_raja || vertailu.is_some() {
                    self.harmaita += 1;
                }
            }
        } else if let Some(maski) = self.laikut_ja_tunnistin(lw, &kr, &ab) {
            // harmaa läikkä tai värittämätön esine värillisen alueen keskellä: täytetään toisen mallin värein,
            // sävy ruudun omasta värityksestä
            if let Some(uusi) = self.varita(mv)? {
                let uusi = siirra_tilastoon(lw, &uusi, &ab_tilasto(lw, &uusi), &ab_tilasto(lw, &ab), 1.0);
                for (j, mm) in maski.data.iter().enumerate() {
                    ab.x.data[j] = ab.x.data[j] * (1.0 - mm) + uusi.x.data[j] * mm;
                    ab.y.data[j] = ab.y.data[j] * (1.0 - mm) + uusi.y.data[j] * mm;
                }
                self.laikkuja += 1;
            }
        }
        // tasalaatuisuus: väritys ei saa hypätä edellisistä saman kohtauksen kuvista
        let mut t = ab_tilasto(lw, &ab);
        if let Some(v) = &vertailu {
            let ero = tilastoero(&t, v);
            if ero > HYPPYRAJA {
                let voima = ((ero - HYPPYRAJA) / ero).clamp(0.0, 0.85);
                ab = siirra_tilastoon(lw, &ab, &t, v, voima);
                t = ab_tilasto(lw, &ab);
                self.tasattuja += 1;
            }
        }
        if ruudun_kroma(lw, &kroma(&ab)) >= self.harmaa_raja {
            self.historia.push_back(t);
            if self.historia.len() > VERTAILUKUVIA {
                self.historia.pop_front();
            }
        }
        Ok(ab)
    }

    /// Sääntöpohjaiset läikät ja tunnistimen löytämät värittämättömät kohdat yhdeksi pehmeäksi maskiksi.
    fn laikut_ja_tunnistin(&mut self, lw: &Taso, kr: &Taso, ab: &Kentta) -> Option<Taso> {
        let mut maski = laikut(lw, kr);
        if let Some(t) = self.tunnistin.as_mut() {
            match t.maski(lw, ab) {
                Ok(Some(m)) => {
                    self.tunnistettuja += 1;
                    maski = Some(match maski {
                        Some(mut a) => {
                            a.data.iter_mut().zip(&m.data).for_each(|(x, y)| *x = x.max(*y));
                            a
                        }
                        None => m,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    loki(&format!("Detector disabled: {e:#}"));
                    self.tunnistin = None;
                }
            }
        }
        maski
    }

    pub fn yhteenveto(&self) -> String {
        format!("filled {} grey frames, patches in {} frames ({} found by the detector), smoothed {} colour jumps",
                self.harmaita, self.laikkuja, self.tunnistettuja, self.tasattuja)
    }
}

/// Valmiin videon tarkistuksen tulos (kuvanumerot).
#[derive(Default)]
pub struct Tarkistus {
    pub kuvia: usize,
    pub harmaat: Vec<usize>,
    pub laikut: Vec<usize>,
    pub hypyt: Vec<usize>,
}

impl Tarkistus {
    pub fn ongelmia(&self) -> usize {
        self.harmaat.len() + self.laikut.len() + self.hypyt.len()
    }
}

/// Kuvanumerot yhtenäisiksi aikaväleiksi raporttia varten.
fn aikavalit(kuvat: &[usize], fps: f64) -> String {
    let mut v: Vec<(usize, usize)> = Vec::new();
    for &k in kuvat {
        match v.last_mut() {
            Some(e) if k <= e.1 + (fps as usize).max(1) => e.1 = k,
            _ => v.push((k, k)),
        }
    }
    let osat: Vec<String> = v.iter().take(40).map(|&(a, b)| {
        if a == b { aikateksti(a as f64 / fps) } else { format!("{}–{}", aikateksti(a as f64 / fps), aikateksti(b as f64 / fps)) }
    }).collect();
    let mut s = osat.join(", ");
    if v.len() > 40 {
        s += &format!(" … (+{} kohtaa)", v.len() - 40);
    }
    s
}

/// Käy valmiin videon läpi kuva kuvalta ja etsii harmaat ruudut, värittämättömät läikät ja värihypyt.
pub fn tarkista_video(polku: &str, tyoleveys: usize, harmaa_raja: f32, leikkaus_raja: f32, seis: &AtomicBool) -> Result<Tarkistus> {
    let t = Tiedot::lue(polku)?;
    let tw = (t.w.min(tyoleveys.max(64)) / 2 * 2).max(2);
    let th = (((t.h as f64 * tw as f64 / t.w as f64).round() as usize) / 2 * 2).max(2);
    let mut lukija = Lukija::avaa(&t, 0, t.n, Some((tw, th)))?;
    let mut leik = Leikkaukset::new(leikkaus_raja);
    let mut tulos = Tarkistus::default();
    let mut edellinen: Option<Tilasto> = None;
    let mut varillisia_kohtauksessa = 0usize;
    let mut ed = crate::tuotanto::Edistys::new("Check", t.n);
    let mut i = 0;
    while let Some(k) = lukija.seuraava() {
        if seis.load(Ordering::Relaxed) {
            bail!("Cancelled.");
        }
        let lab = bgr_lab(&k);
        let leikkaus = leik.onko(&pieni_harmaa(&lab.l));
        if leikkaus {
            edellinen = None;
            varillisia_kohtauksessa = 0;
        }
        let kr = kroma(&lab.ab);
        let rk = ruudun_kroma(&lab.l, &kr);
        let valo = keskivalo(&lab.l);
        if rk < harmaa_raja {
            // harmaa ruutu värillisessä kohtauksessa (ei häivytystä mustaan tai valkoiseen)
            if varillisia_kohtauksessa >= 3 && valo > 10.0 && valo < 92.0 {
                tulos.harmaat.push(i);
            }
        } else {
            varillisia_kohtauksessa += 1;
            if laikut(&lab.l, &kr).is_some() {
                tulos.laikut.push(i);
            }
            let ti = ab_tilasto(&lab.l, &lab.ab);
            if let Some(e) = &edellinen {
                if !leikkaus && tilastoero(&ti, e) > 2.0 * HYPPYRAJA {
                    tulos.hypyt.push(i);
                }
            }
            edellinen = Some(ti);
        }
        i += 1;
        ed.paivita(i);
    }
    tulos.kuvia = i;
    ed.valmis();
    let fps = t.fps;
    loki(&format!("Check {}: {} frames – grey frames {}, patches {}, colour jumps {}.",
                  Path::new(polku).file_name().unwrap_or_default().to_string_lossy(), i, tulos.harmaat.len(), tulos.laikut.len(), tulos.hypyt.len()));
    for (nimi, v) in [("grey frames", &tulos.harmaat), ("patches", &tulos.laikut), ("colour jumps", &tulos.hypyt)] {
        if !v.is_empty() {
            loki(&format!("  {nimi}: {}", aikavalit(v, fps)));
        }
    }
    Ok(tulos)
}

/// Värin välkyntä kohtausten sisällä (keskiarvo ja 95. persentiili kuvaparia kohden, Lab-yksikköä).
pub fn valkyntamittari(polku: &str, leikkaus_raja: f32, seis: &AtomicBool) -> Result<(f32, f32, usize)> {
    let t = Tiedot::lue(polku)?;
    let (w, h) = t.koko_enintaan(320);
    let mut leik = Leikkaukset::new(leikkaus_raja);
    let mut ed: Option<(crate::kuva::Taso, crate::kuva::Kentta)> = None;
    let mut arvot = Vec::new();
    for (i, k) in Lukija::avaa(&t, 0, t.n, Some((w, h)))?.enumerate() {
        if i % 64 == 0 && seis.load(Ordering::Relaxed) {
            bail!("Cancelled.");
        }
        let lab = bgr_lab(&k);
        let g = pieni_harmaa(&lab.l);
        let leikkaus = leik.onko(&g);
        if let (Some((ge, abe)), false) = (&ed, leikkaus) {
            if let Some(v) = crate::vakautus::valkynta(&g, &lab.ab, ge, abe) {
                arvot.push(v);
            }
        }
        ed = Some((g, lab.ab));
    }
    if arvot.is_empty() {
        return Ok((0.0, 0.0, 0));
    }
    let ka = arvot.iter().sum::<f32>() / arvot.len() as f32;
    let mut j = arvot.clone();
    j.sort_by(f32::total_cmp);
    Ok((ka, j[(j.len() * 95 / 100).min(j.len() - 1)], arvot.len()))
}

fn valitiedosto(ulos: &str) -> String {
    let p = Path::new(ulos);
    p.with_file_name(format!("{}.vaihe1.mkv", p.file_stem().unwrap_or_default().to_string_lossy())).to_string_lossy().into_owned()
}

/// Koko putki. `a.tila` = "putki", `a.video` = mustavalko-originaali. Palauttaa lopullisen tiedoston.
pub fn aja(mut a: Asetukset, seis: Arc<AtomicBool>) -> Result<String> {
    if a.ulos.is_empty() {
        let p = Path::new(&a.video);
        a.ulos = p.with_file_name(format!("{}_varitetty.mkv", p.file_stem().unwrap().to_string_lossy())).to_string_lossy().into();
    }
    let vali = valitiedosto(&a.ulos);
    let tulos = vaiheet(a, seis, &vali);
    if tulos.is_err() {
        // keskeytetty tai epäonnistunut ajo ei jätä välitiedostoa
        let _ = std::fs::remove_file(&vali);
        let _ = std::fs::remove_file(format!("{vali}.ffmpeg.log"));
    }
    tulos
}

fn vaiheet(a: Asetukset, seis: Arc<AtomicBool>, vali: &str) -> Result<String> {
    let t0 = std::time::Instant::now();
    let lahde = a.video.clone();
    let lopullinen = a.ulos.clone();
    let vali = vali.to_string();
    let (tyoleveys, harmaa_raja, leikkaus_raja) = (a.tyoleveys, a.harmaa_raja, a.leikkaus_raja);

    // 1. väritys laadunvalvonnalla
    loki("━━ Step 1/3: colorizing (patch filling and comparison with previous frames)");
    let mut v = a.clone();
    v.tila = "vari".into();
    v.ulos = vali.clone();
    v.valvonta = true;
    v.crf = v.crf.min(10); // välitulos lähes häviöttömänä, koska se pakataan vielä kerran
    let mut tuottaja = Tuottaja::new(v, seis.clone())?;
    let (alku_s, kesto_s) = (tuottaja.alku_s(), tuottaja.kesto_s());
    tuottaja.aja()?;

    // 2. tarkistus
    loki("━━ Step 2/3: frame-by-frame check of the intermediate result");
    let tarkistus = tarkista_video(&vali, tyoleveys, harmaa_raja, leikkaus_raja, &seis)?;

    // 3. korjaus
    if tarkistus.ongelmia() == 0 {
        loki("━━ Step 3/3: nothing to repair – the intermediate result is the final video.");
        let _ = std::fs::remove_file(&lopullinen);
        std::fs::rename(&vali, &lopullinen)?;
    } else {
        loki(&format!("━━ Step 3/3: repair against the black-and-white original ({} problem frames)", tarkistus.ongelmia()));
        let mut k = a.clone();
        k.tila = "korjaa".into();
        k.video = vali.clone();
        k.alkuperainen = lahde.clone();
        k.alkup_siirto_s = alku_s;
        k.ulos = lopullinen.clone();
        k.alku_s = 0.0;
        k.kesto_s = 0.0;
        k.tayta_harmaat = true;
        k.valvonta = true;
        // ääni: välitulos sisältää jo oikean pätkän lähteen äänestä
        if k.aanilahde == lahde {
            k.aanilahde.clear();
        }
        let _ = kesto_s;
        Tuottaja::new(k, seis.clone())?.aja()?;
        loki("Check of the final result:");
        let jalkeen = tarkista_video(&lopullinen, tyoleveys, harmaa_raja, leikkaus_raja, &seis)?;
        loki(&format!("Repair: problem frames {} → {}.", tarkistus.ongelmia(), jalkeen.ongelmia()));
        let _ = std::fs::remove_file(&vali);
    }
    edistys("Pipeline done.", 1, 1);
    loki(&format!("Pipeline done ({}): {lopullinen}", &aikateksti(t0.elapsed().as_secs_f64())[..7]));
    Ok(lopullinen)
}
