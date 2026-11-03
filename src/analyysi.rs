//! Kohtausanalyysi: jokaiseen kohtaukseen kiinnipiste, ehdotukset (DDColor-tyylit, referenssillä myös
//! B ja E) ja näkömallin valitsema paras väritystapa. Tulos tallennetaan Exemplar-projektiin.
//! Käsin valittuihin kiinnipisteisiin ei kosketa, ja keskeytetty analyysi jatkuu siitä mihin jäi.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};

use crate::kuva::{Bgr, Kentta};
use crate::nako;
use crate::resepti::{KIRJAIMET, Kiinnipiste, Projekti, Referenssit, Resepti, ehdokkaat};
use crate::tuotanto::{Edistys, aja_resepti, kohtausrajat};
use crate::vari::lab_bgr;
use crate::video::{Tiedot, aikateksti, hae_kehys};
use crate::{edistys, loki};

#[derive(Clone)]
pub struct AnalyysiAsetukset {
    pub leikkaus_raja: f32,
    pub ddcolor: String,
    pub laite: String,
    pub nako_malli: String,
    /// lyhyemmät kohtaukset saavat värinsä naapureista (interpolointi)
    pub min_kesto_s: f64,
    /// enintään näin monta uutta analyysiä (0 = kaikki)
    pub enintaan: usize,
}

pub struct Yhteenveto {
    pub kohtauksia: usize,
    pub analysoitu: usize,
    pub lyhyita: usize,
    /// kohtaukset, joissa on jo valittu kiinnipiste (käsin tai aiemmassa analyysissä)
    pub valmiiksi_valittuja: usize,
    pub virheita: usize,
}

pub fn kuvakansio(projekti: &str) -> PathBuf {
    let p = Path::new(projekti);
    p.with_file_name(format!("{}_kuvat", p.file_stem().unwrap_or_default().to_string_lossy()))
}

/// Ajaa analyysin. `tulos` kutsutaan jokaisesta valmiista kiinnipisteestä (tallennusta varten).
pub fn kohtausanalyysi(p: &Projekti, kuvat: &Path, a: &AnalyysiAsetukset, seis: &AtomicBool,
                       tulos: &mut dyn FnMut(Kiinnipiste)) -> Result<Yhteenveto> {
    let t = Tiedot::lue(&p.video)?;
    loki("Scene analysis: finding scene cuts…");
    let rajat = kohtausrajat(&t, a.leikkaus_raja, 0, 0, seis)?;
    let mut reunat = vec![0usize];
    reunat.extend(&rajat);
    reunat.push(t.n);
    let kohtaukset: Vec<(usize, usize)> = reunat.windows(2).filter(|w| w[1] > w[0]).map(|w| (w[0], w[1])).collect();
    let min_kuvia = (a.min_kesto_s * t.fps).round() as usize;
    let _ = std::fs::create_dir_all(kuvat);
    let refit = Referenssit::default();
    let mut y = Yhteenveto { kohtauksia: kohtaukset.len(), analysoitu: 0, lyhyita: 0, valmiiksi_valittuja: 0, virheita: 0 };
    let tehtavat: Vec<(usize, usize, Option<&Kiinnipiste>)> = kohtaukset
        .iter()
        .filter_map(|&(alku, loppu)| {
            if loppu - alku < min_kuvia {
                y.lyhyita += 1;
                return None;
            }
            let olemassa: Vec<&Kiinnipiste> = p.kiinnipisteet.iter().filter(|k| k.kehys >= alku && k.kehys < loppu).collect();
            if olemassa.iter().any(|k| !k.valinta.is_empty()) {
                y.valmiiksi_valittuja += 1;
                return None; // valinta on jo tehty (käsin tai aiemmassa analyysissä)
            }
            Some((alku, loppu, olemassa.first().copied()))
        })
        .collect();
    let n = if a.enintaan > 0 { tehtavat.len().min(a.enintaan) } else { tehtavat.len() };
    loki(&format!("Scenes {}, to analyse {} (short ones under {:.1} s: {}, already chosen: {}).",
                  y.kohtauksia, n, a.min_kesto_s, y.lyhyita, y.valmiiksi_valittuja));
    // Kaksi vaihetta, jotta väritysmallit (ONNX/DirectML) ja näkömalli (Ollama) eivät tarvitse
    // näytönohjaimen muistia yhtä aikaa: 12 Gt ei riitä molemmille.
    let ehdotuskansio = kuvat.join("analyysi");
    let _ = std::fs::create_dir_all(&ehdotuskansio);
    let koko = t.koko_enintaan(640);
    let mut valmiit: Vec<(usize, usize, String, Vec<(String, Resepti, PathBuf)>)> = Vec::new();
    let mut ed = Edistys::new("Scene analysis 1/2 (suggestions)", n);
    for (i, &(alku, loppu, olemassa)) in tehtavat.iter().take(n).enumerate() {
        if seis.load(Ordering::Relaxed) {
            bail!("Cancelled during the suggestion step ({i}/{n}); computed suggestions are kept and resume next time.");
        }
        // kohtauksen keskikohta tai siinä jo oleva (valitsematon) kiinnipiste
        let kehys = olemassa.map(|k| k.kehys).unwrap_or((alku + loppu) / 2);
        let referenssi = olemassa.map(|k| k.referenssi.clone()).unwrap_or_default();
        match laske_ehdotukset(&t, kehys, &referenssi, koko, a, &refit, &ehdotuskansio) {
            Ok(e) => valmiit.push((kehys, loppu - alku, referenssi, e)),
            Err(e) => {
                y.virheita += 1;
                loki(&format!("  {}: suggestions failed: {e:#}", aikateksti(t.aika(kehys))));
            }
        }
        ed.paivita(i + 1);
    }
    crate::mallit::vapauta();
    let n2 = valmiit.len();
    let mut ed = Edistys::new("Scene analysis 2/2 (vision model)", n2);
    for (i, (kehys, kesto, referenssi, ehdotukset)) in valmiit.into_iter().enumerate() {
        if seis.load(Ordering::Relaxed) {
            nako::vapauta_malli(&a.nako_malli);
            bail!("Cancelled ({i}/{n2} scenes chosen, results saved; resumes next time).");
        }
        match valitse(kehys, &referenssi, ehdotukset, a) {
            Ok((kp, kuva)) => {
                let _ = std::fs::copy(&kuva, kuvat.join(format!("kp_{kehys:07}.png")));
                if let Ok(i) = image::open(&kuva) {
                    let _ = i.to_rgb8().save(kuvat.join(format!("kp_{kehys:07}.jpg")));
                }
                loki(&format!("  {} ({:.1} s) {}: {} – {}", aikateksti(t.aika(kehys)), kesto as f64 / t.fps,
                              kp.valinta, kp.resepti.kuvaus(), kp.analyysi));
                tulos(kp);
                y.analysoitu += 1;
            }
            Err(e) => {
                y.virheita += 1;
                loki(&format!("  {}: the vision model's choice failed: {e:#}", aikateksti(t.aika(kehys))));
                if y.virheita >= 5 && y.analysoitu == 0 {
                    nako::vapauta_malli(&a.nako_malli);
                    bail!("The analysis failed repeatedly: {e:#}");
                }
            }
        }
        ed.paivita(i + 1);
    }
    nako::vapauta_malli(&a.nako_malli);
    edistys(&format!("Scene analysis done: {}/{n} scenes", y.analysoitu), n, n.max(1));
    Ok(y)
}

/// Laskee kohtauksen ehdotukset ja tallentaa ne kansioon (olemassa olevat käytetään uudelleen).
fn laske_ehdotukset(t: &Tiedot, kehys: usize, referenssi: &str, koko: (usize, usize), a: &AnalyysiAsetukset,
                    refit: &Referenssit, kansio: &Path) -> Result<Vec<(String, Resepti, PathBuf)>> {
    let reseptit: Vec<(String, Resepti)> = KIRJAIMET
        .iter()
        .zip(ehdokkaat(&a.ddcolor, referenssi, 0.7))
        .filter_map(|(k, r)| r.map(|r| (k.to_string(), r)))
        .collect();
    let polku = |k: &str| kansio.join(format!("kp_{kehys:07}_{}_{k}.png", a.ddcolor));
    if reseptit.iter().all(|(k, _)| polku(k).is_file()) {
        return Ok(reseptit.into_iter().map(|(k, r)| { let p = polku(&k); (k, r, p) }).collect());
    }
    let kuva = hae_kehys(t, kehys, Some(koko))?;
    let mut ab: HashMap<String, Kentta> = HashMap::new();
    let mut ulos = Vec::new();
    for (k, r) in reseptit {
        let (l, abr) = aja_resepti(&r, &kuva, &mut ab, &a.laite, refit)?;
        let b = lab_bgr(&l, &abr);
        let p = polku(&k);
        image::save_buffer(&p, &b.rgb(), b.w as u32, b.h as u32, image::ExtendedColorType::Rgb8)?;
        ulos.push((k, r, p));
    }
    Ok(ulos)
}

fn valitse(kehys: usize, referenssi: &str, ehdotukset: Vec<(String, Resepti, PathBuf)>, a: &AnalyysiAsetukset)
           -> Result<(Kiinnipiste, PathBuf)> {
    let mut vertailu = Vec::new();
    for (k, r, p) in &ehdotukset {
        let i = image::open(p)?.to_rgb8();
        vertailu.push((k.clone(), r.kuvaus(), Bgr::rgb_kuvasta(i.width() as usize, i.height() as usize, i.as_raw())));
    }
    let s = nako::suosittele_vakaasti(&a.nako_malli, &vertailu, true)?;
    let (kirjain, r, p) = ehdotukset.into_iter().find(|e| e.0 == s.paras).expect("suositus on ehdotusten joukossa");
    Ok((Kiinnipiste {
        kehys,
        referenssi: referenssi.to_string(),
        valinta: kirjain.clone(),
        resepti: r,
        f_pohja: kirjain,
        analyysi: format!("{}: {}", if s.kohtaus.is_empty() { "scene" } else { &s.kohtaus }, s.perustelu),
    }, p))
}
