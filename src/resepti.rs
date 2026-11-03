//! Resepti = yksi väritystapa: perusmenetelmä + valinnainen referenssi + käsisäätö (H/S/V).
//! Resepti on deterministinen, joten sen voi ajaa mille tahansa kehykselle: exemplar-tilassa
//! kiinnipisteen valittu versio on resepti, ja välit interpoloidaan reseptien välillä.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::kuva::{Kentta, Taso};
use crate::mallit;
use crate::vari::{Referenssi, ab_tilasto, hsv_saato, siirra_tilastoon};

pub const SIIRTO: &str = "siirto";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Resepti {
    pub menetelma: String,
    pub referenssi: String,
    pub ref_voima: f32,
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub nimi: String,
}

impl Default for Resepti {
    fn default() -> Self {
        Resepti { menetelma: "ddcolor_suomi".into(), referenssi: String::new(), ref_voima: 0.7, h: 0.0, s: 1.0, v: 1.0, nimi: String::new() }
    }
}

impl Resepti {
    /// Varoitus, jos sävyn tai kylläisyyden säätö vääristää ihonvärit (esim. H -180 tekee kasvoista siniset).
    pub fn saatovaroitus(&self) -> Option<String> {
        let mut v = Vec::new();
        if self.h.abs() > 25.0 {
            v.push(format!("hue H {:+.0}° rotates all colours (skin turns {})", self.h,
                           if self.h.abs() > 90.0 { "blue/green" } else if self.h > 0.0 { "yellow-green" } else { "red-violet" }));
        }
        if self.s > 1.4 {
            v.push(format!("saturation S {:.2} makes skin oversaturated", self.s));
        }
        if self.s < 0.5 {
            v.push(format!("saturation S {:.2} leaves the image almost grey", self.s));
        }
        if v.is_empty() { None } else { Some(v.join("; ")) }
    }

    pub fn uusi(menetelma: &str, nimi: &str) -> Self {
        Resepti { menetelma: menetelma.into(), nimi: nimi.into(), ..Default::default() }
    }

    pub fn mallit(&self) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        if self.menetelma != SIIRTO {
            s.insert(self.menetelma.clone());
        }
        s
    }

    pub fn kuvaus(&self) -> String {
        let mut t = match mallit::tieto(&self.menetelma) {
            Some(_) => mallit::kuvaus(&self.menetelma).unwrap_or_default(),
            None => "Colour transfer from a reference".to_string(),
        };
        if !self.referenssi.is_empty() && self.menetelma != SIIRTO {
            t += &format!(" + reference {:.0} %", self.ref_voima * 100.0);
        }
        if (self.h, self.s, self.v) != (0.0, 1.0, 1.0) {
            t += &format!(" H{:+.0} S{:.2} V{:.2}", self.h, self.s, self.v);
        }
        t
    }
}

/// Referenssikuvien välimuisti.
#[derive(Default, Clone)]
pub struct Referenssit(Arc<Mutex<HashMap<String, Arc<Referenssi>>>>);

impl Referenssit {
    pub fn hae(&self, polku: &str) -> Result<Option<Arc<Referenssi>>> {
        if polku.is_empty() {
            return Ok(None);
        }
        if let Some(r) = self.0.lock().unwrap().get(polku) {
            return Ok(Some(r.clone()));
        }
        let r = Arc::new(Referenssi::tiedostosta(polku)?);
        self.0.lock().unwrap().insert(polku.to_string(), r.clone());
        Ok(Some(r))
    }
}

/// Palauttaa (L, ab) reseptin mukaan. `mallien_ab`: mallin nimi -> ab tälle kehykselle.
pub fn sovella(r: &Resepti, l: &Taso, mallien_ab: &HashMap<String, Kentta>, refit: &Referenssit) -> Result<(Taso, Kentta)> {
    let viite = refit.hae(&r.referenssi)?;
    let ab = if r.menetelma == SIIRTO {
        viite.as_ref().ok_or_else(|| anyhow!("Colour transfer needs a reference image."))?.siirra_harmaaseen(l)
    } else {
        let ab = mallien_ab.get(&r.menetelma).ok_or_else(|| anyhow!("result of model {} is missing", r.menetelma))?;
        match &viite {
            Some(v) if r.ref_voima > 0.0 => siirra_tilastoon(l, ab, &ab_tilasto(l, ab), &v.tilasto, r.ref_voima),
            _ => ab.clone(),
        }
    };
    Ok(hsv_saato(l, &ab, r.h, r.s, r.v))
}

pub const KIRJAIMET: [&str; 6] = ["A", "B", "C", "D", "E", "F"];

/// Kiinnipisteen ehdotukset A–E (F = käsisäätö lasketaan erikseen). Ilman referenssiä B ja E puuttuvat.
pub fn ehdokkaat(ddcolor: &str, referenssi: &str, ref_voima: f32) -> Vec<Option<Resepti>> {
    let ref_ = !referenssi.is_empty();
    vec![
        Some(Resepti::uusi(ddcolor, "A: DDColor (automatic)")),
        ref_.then(|| Resepti { referenssi: referenssi.into(), ref_voima, ..Resepti::uusi(ddcolor, "B: DDColor + reference") }),
        Some(Resepti::uusi("ddcolor_suomi_technicolor", "C: Technicolor style")),
        Some(Resepti::uusi("ddcolor_suomi_eastmancolor", "D: Eastmancolor style")),
        ref_.then(|| Resepti { referenssi: referenssi.into(), ..Resepti::uusi(SIIRTO, "E: Colour transfer (palette)") }),
    ]
}

// --------------------------------------------------------------------------
// Projekti
// --------------------------------------------------------------------------
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Kiinnipiste {
    pub kehys: usize,
    pub referenssi: String,
    pub valinta: String,
    pub resepti: Resepti,
    pub f_pohja: String,
    /// kohtausanalyysin tulos (kohtauksen tyyppi ja näkömallin perustelu); tyhjä = käsin valittu
    pub analyysi: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Projekti {
    pub versio: u32,
    pub video: String,
    pub kiinnipisteet: Vec<Kiinnipiste>,
}

impl Default for Projekti {
    fn default() -> Self {
        Projekti { versio: 1, video: String::new(), kiinnipisteet: Vec::new() }
    }
}

impl Projekti {
    pub fn lue(polku: &str) -> Result<Projekti> {
        let t = std::fs::read_to_string(polku)?;
        let p: Projekti = serde_json::from_str(t.trim_start_matches('\u{feff}'))?;
        if p.video.is_empty() {
            return Err(anyhow!("not a bw2color project"));
        }
        Ok(p)
    }
    pub fn tallenna(&self, polku: &str) -> Result<()> {
        std::fs::write(polku, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
