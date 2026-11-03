//! Käyttöliittymän itsetesti (vain `--features itsetesti`): käy läpi käyttäjän työnkulun kuten nappeja
//! painamalla, odottaa jokaisen vaiheen valmistumisen ja kirjoittaa raportin. Ajo: VARITA_TESTI=<kansio>.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{Sovellus, Valilehti};
use crate::resepti::Projekti;
use crate::video::Tiedot;

type Toimi = Box<dyn FnMut(&mut Sovellus)>;
type Ehto = Box<dyn FnMut(&mut Sovellus) -> bool>;
type Tarkistus = Box<dyn FnMut(&mut Sovellus) -> (bool, String)>;

struct Askel {
    nimi: &'static str,
    tee: Toimi,
    odota: Ehto,
    tarkista: Tarkistus,
    aikaraja: u64,
}

pub struct Testi {
    askeleet: Vec<Askel>,
    i: usize,
    aloitettu: Option<Instant>,
    raportti: Vec<String>,
    kansio: PathBuf,
    virheita: usize,
}

/// Testivideoiden kansio käännösaikana (BW2COLOR_TESTIVIDEOT); videot eivät kuulu repoon.
const D: &str = match option_env!("BW2COLOR_TESTIVIDEOT") {
    Some(d) => d,
    None => r"C:\testivideot",
};

fn a(nimi: &'static str, aikaraja: u64, tee: impl FnMut(&mut Sovellus) + 'static, odota: impl FnMut(&mut Sovellus) -> bool + 'static,
     tarkista: impl FnMut(&mut Sovellus) -> (bool, String) + 'static) -> Askel {
    Askel { nimi, tee: Box::new(tee), odota: Box::new(odota), tarkista: Box::new(tarkista), aikaraja }
}

fn kuvia(p: &PathBuf) -> usize {
    Tiedot::lue(&p.to_string_lossy()).map(|t| t.n).unwrap_or(0)
}

fn kaappaa(kansio: &PathBuf, nimi: &str) -> impl FnMut(&mut Sovellus) -> bool + 'static {
    // ulkoinen skripti ottaa kuvan, kun merkkitiedosto ilmestyy, ja poistaa sen
    let merkki = kansio.join(format!("kaappaa_{nimi}.txt"));
    let mut luotu: Option<Instant> = None;
    move |_| {
        if luotu.is_none() {
            let _ = std::fs::write(&merkki, "");
            luotu = Some(Instant::now());
        }
        !merkki.exists() || luotu.unwrap().elapsed() > Duration::from_secs(15)
    }
}

impl Testi {
    pub fn new(kansio: PathBuf) -> Testi {
        let _ = std::fs::create_dir_all(&kansio);
        let bw = format!(r"{D}\5 Presidentin Vävy_bw_final.mp4");
        let var = format!(r"{D}\5 Presidentin Vävy_bw_final_varitetty.mkv");
        let veljet = format!(r"{D}\noin 7 veljestä.mp4");
        let ref_kuva = kansio.join("referenssi.png");
        let k = kansio.clone();
        let proj_var = k.join("varitetty.bw2color.json");
        let proj = k.join("projekti.bw2color.json");
        let (k1, k2, k3, k4, k5, k6) = (k.clone(), k.clone(), k.clone(), k.clone(), k.clone(), k.clone());
        let (bw1, bw2, var1) = (bw.clone(), bw.clone(), var.clone());
        let (v1, v2) = (veljet.clone(), veljet.clone());
        let (pv, p1, p2) = (proj_var.clone(), proj.clone(), proj.clone());
        let ref1 = ref_kuva.clone();
        let alku = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let alku2 = alku.clone();
        let askeleet = vec![
            a("väärä kohtausraja korjautuu", 5, |s| { s.s.leikkaus_raja = 14.0; }, |_| true, |s| {
                let e = s.rakenna_ajo();
                s.s.leikkaus_raja = 0.35;
                (matches!(e, Err(ref t) if t.contains("between 0.05 and 0.95")), format!("{:?}", e.err()))
            }),
            a("väritetyn videon projekti -> varoitus", 60, move |s| {
                let _ = Projekti { versio: 1, video: var1.clone(), kiinnipisteet: vec![] }.tallenna(&pv.to_string_lossy());
                s.valilehti = Valilehti::Exemplar;
                s.tila_valilehti = Valilehti::Exemplar;
                s.avaa_projekti(&pv.to_string_lossy());
            }, |s| s.kehys.is_some() && s.tarkistettu_video.contains("varitetty"), |s| (s.e_varoitus.contains("already in colour"), s.e_varoitus.clone())),
            a("mustavalkoprojekti: ei varoitusta, ohje näkyy", 60, move |s| {
                s.projekti = Projekti { versio: 1, video: bw1.clone(), kiinnipisteet: vec![] };
                s.s.e_projekti = p1.to_string_lossy().into();
                s.s.e_ulos = k1.join("exemplar.mkv").to_string_lossy().into();
                s.tallenna_projekti();
                s.lataa_projektin_video();
            }, |s| s.kehys.is_some() && s.tarkistettu_video.contains("bw_final.mp4"), |s| {
                (s.e_varoitus.is_empty() && s.ohje().contains("No keyframes"), s.ohje())
            }),
            a("Tuota ilman kiinnipisteitä -> automaattiset (5)", 300, |s| { s.s.e_automaattisia = 5; s.aloita(); },
              |s| !s.tyo_kaynnissa && !s.projekti.kiinnipisteet.is_empty() && s.ehdotus_kehys == Some(s.kohta) && s.ehdotukset.len() >= 4,
              |s| (s.projekti.kiinnipisteet.len() == 5 && s.ajo.is_none(),
                   format!("{:?}", s.projekti.kiinnipisteet.iter().map(|p| p.kehys).collect::<Vec<_>>()))),
            a("ehdotukset A, C, D, F syntyivät", 5, |_| {}, |_| true, |s| {
                let mut k: Vec<_> = s.ehdotukset.keys().cloned().collect();
                k.sort();
                (["A", "C", "D", "F"].iter().all(|x| s.ehdotukset.contains_key(*x)), format!("{k:?}"))
            }),
            a("kiinnipisteet tallentuivat", 5, |_| {}, |_| true, move |_| {
                let n = Projekti::lue(&p2.to_string_lossy()).map(|p| p.kiinnipisteet.len()).unwrap_or(0);
                (n == 5, format!("{n}"))
            }),
            a("selaus: kuva ▶, ◀ 1 s, ⏭, ⏮, nuoli, aika", 30, move |s| {
                alku.set(s.kohta);
                s.askel(1);
            }, |_| true, move |s| {
                let alku = alku2.get();
                let mut ok = s.kohta == alku + 1;
                s.askel(-25);
                ok &= s.kohta == alku + 1 - 25.min(alku + 1);
                s.askel(i64::MAX);
                ok &= s.kohta == s.e_tiedot.as_ref().unwrap().n - 1;
                s.askel(i64::MIN);
                ok &= s.kohta == 0;
                s.e_aika = "0:01:00".into();
                if let (Some(t), Some(x)) = (&s.e_tiedot, crate::video::aika_sekunneiksi(&s.e_aika)) {
                    let f = t.kehys(x);
                    s.siirry(f);
                }
                ok &= s.kohta == 1500;
                (ok, format!("kohta {}", s.kohta))
            }),
            a("Seuraava ▶ kiinnipisteeseen + ehdotukset", 300, |s| s.hyppaa(1),
              |s| s.ehdotus_kehys == Some(s.kohta) && !s.tyo_kaynnissa && s.ehdotukset.contains_key("F"),
              |s| (s.kp_tassa().is_some(), format!("kohta {}", s.kohta))),
            a("näkömalli suosittelee ehdotuksen", 300, |s| s.s.nako_suosittele = true,
              |s| matches!(&s.suositus, Some((f, _)) if *f == s.kohta) && s.suositus_kaynnissa.is_none(), |s| match &s.suositus {
                Some((_, Ok(r))) => (s.ehdotukset.contains_key(&r.paras) && ["A", "B", "C", "D", "E"].contains(&r.paras.as_str()),
                                     format!("{} (toinen {}), {:.1} s: {}", r.paras, r.toinen, r.aika_s, r.perustelu)),
                Some((_, Err(e))) => (false, e.clone()),
                None => (false, "ei suositusta".into()),
            }),
            a("näkömalli valitsee automaattisesti", 300, |s| {
                s.s.nako_valitse = true;
                let f = s.projekti.kiinnipisteet.iter().find(|p| p.valinta.is_empty() && p.kehys != s.kohta).map(|p| p.kehys).unwrap();
                s.siirry(f);
            }, |s| matches!(&s.suositus, Some((f, _)) if *f == s.kohta) && s.suositus_kaynnissa.is_none(), |s| {
                s.s.nako_valitse = false;
                let i = s.kp_tassa().unwrap();
                let valinta = s.projekti.kiinnipisteet[i].valinta.clone();
                let paras = match &s.suositus { Some((_, Ok(r))) => r.paras.clone(), _ => String::new() };
                let ok = !valinta.is_empty() && valinta == paras;
                // takaisin edelliseen kiinnipisteeseen jatkotestejä varten
                s.hyppaa(-1);
                (ok, format!("valittu {valinta}, suositus {paras}"))
            }),
            a("takaisin kiinnipisteeseen", 300, |_| {}, |s| s.ehdotus_kehys == Some(s.kohta) && !s.tyo_kaynnissa && s.ehdotukset.contains_key("F"),
              |s| (s.kp_tassa().is_some(), format!("kohta {}", s.kohta))),
            a("valitse C", 5, |s| s.valitse_ehdotus("C"), |_| true, |s| {
                let i = s.kp_tassa().unwrap();
                (s.projekti.kiinnipisteet[i].valinta == "C", s.projekti.kiinnipisteet[i].resepti.kuvaus())
            }),
            a("referenssi -> B ja E syntyvät", 300, move |s| {
                // referenssikuvaksi väritetyn videon kehys
                if let Ok(t) = Tiedot::lue(&var) {
                    if let Ok(k) = crate::video::hae_kehys(&t, 1000, None) {
                        let _ = image::save_buffer(&ref1, &k.rgb(), k.w as u32, k.h as u32, image::ExtendedColorType::Rgb8);
                    }
                }
                let i = s.kp_tassa().unwrap();
                s.projekti.kiinnipisteet[i].referenssi = ref_kuva.to_string_lossy().into();
                s.ehdotus_kehys = None;
                s.luo_ehdotukset();
            }, |s| !s.tyo_kaynnissa && s.ehdotukset.contains_key("B") && s.ehdotukset.contains_key("E"), |s| {
                let mut k: Vec<_> = s.ehdotukset.keys().cloned().collect();
                k.sort();
                (k.len() == 6, format!("{k:?}"))
            }),
            a("F (HSV) valitaan ja tallentuu", 10, |s| {
                s.hsv = (10.0, 1.6, 1.0);
                s.hsv_muuttui = true;
            }, |s| !s.hsv_muuttui && s.ehdotukset.get("F").map(|e| (e.resepti.s - 1.6).abs() < 1e-4).unwrap_or(false), move |s| {
                s.valitse_ehdotus("F");
                let p = Projekti::lue(&s.s.e_projekti).unwrap();
                let kp = p.kiinnipisteet.iter().find(|p| p.kehys == s.kohta).unwrap();
                (kp.valinta == "F" && (kp.resepti.s - 1.6).abs() < 1e-4 && kp.resepti.menetelma == "ddcolor_suomi_technicolor", kp.resepti.kuvaus())
            }),
            a("näytä valittu versio", 10, |s| s.nayta_vari = true, |_| true, |s| {
                let ctx = egui::Context::default();
                let ok = s.valittu_kuva(&ctx).is_some();
                (ok, String::new())
            }),
            a("kuvakaappaus: exemplar", 20, |s| s.nayta_vari = false, kaappaa(&k2, "exemplar"), |_| (true, String::new())),
            a("+ Merkitse tämä kuva ja Poista", 300, |s| s.siirry(100),
              |s| s.kohta == 100 && s.kehys.is_some() && !s.haku_kaynnissa.load(Ordering::Relaxed), |s| {
                let n0 = s.projekti.kiinnipisteet.len();
                s.lisaa_kiinnipiste();
                let lisatty = s.projekti.kiinnipisteet.len() == n0 + 1;
                s.poista_kiinnipiste();
                (lisatty && s.projekti.kiinnipisteet.len() == n0, format!("{n0}"))
            }),
            a("Kohtausanalyysi (3 kohtausta), käsin valitut säilyvät", 900, {
                move |s: &mut Sovellus| {
                    s.s.analyysi_enintaan = 3;
                    s.s.analyysi_min_kesto = 1.0;
                    s.aloita_analyysi();
                }
            }, |s| s.analyysi_ajo.is_none(), {
                move |s: &mut Sovellus| {
                    let analysoidut: Vec<String> = s.projekti.kiinnipisteet.iter().filter(|k| !k.analyysi.is_empty())
                        .map(|k| format!("{} {}: {}", k.kehys, k.valinta, &k.analyysi[..k.analyysi.len().min(40)])).collect();
                    let tallennettu = Projekti::lue(&s.s.e_projekti).map(|p| p.kiinnipisteet.iter().filter(|k| !k.analyysi.is_empty()).count()).unwrap_or(0);
                    let kasin = s.projekti.kiinnipisteet.iter().filter(|k| k.analyysi.is_empty() && !k.valinta.is_empty()).count();
                    (analysoidut.len() == 3 && tallennettu == 3 && kasin >= 2,
                     format!("{} analysoitu, {kasin} käsin valittua säilyi; {}", analysoidut.len(), analysoidut.join(" | ")))
                }
            }),
            a("Tuota lopullinen video (näyte 4 s)", 600, |s| {
                let f = s.projekti.kiinnipisteet[1].kehys as f64 / 25.0;
                s.s.nayte_alku = (f - 2.0).max(0.0);
                s.s.nayte_kesto = 4.0;
                s.tila_valilehti = Valilehti::Exemplar;
                s.aloita();
            }, |s| s.ajo.is_none() && !s.tyo_kaynnissa, move |s| {
                let n = kuvia(&k3.join("exemplar.mkv"));
                (n == 100 && s.tila.lock().unwrap().0 == "Done.", format!("{n} kuvaa, {}", s.tila.lock().unwrap().0))
            }),
            a("Avustaja: laaturaportti tuotetusta videosta", 600, |s| {
                s.valilehti = Valilehti::Avustaja;
                s.s.laatu_video = s.viimeisin_tulos.clone();
                s.s.nako_naytteita = 10;
                s.aloita_laatu();
            }, |s| s.laatu_ajo.is_none(), |s| match &s.laatu_tulos {
                Some((html, h)) => (h.len() == 10 && html.is_file(),
                                    format!("{} näytettä, {} ongelmaa, {}", h.len(), h.iter().filter(|x| !x.kunnossa).count(), html.display())),
                None => (false, s.tila.lock().unwrap().0.clone()),
            }),
            a("kuvakaappaus: avustaja", 20, |_| {}, kaappaa(&kansio, "avustaja"), |_| (true, String::new())),
            a("Väritä (putki + kopioruutujen poisto 30 -> 25 fps): noin 7 veljestä, 3 s", 600, move |s| {
                s.valilehti = Valilehti::Varita;
                s.s.v_putki = true;
                s.s.v_kopiot = true;
                s.tila_valilehti = Valilehti::Varita;
                s.s.v_video = v1.clone();
                s.s.v_ulos = k4.join("veljet.mkv").to_string_lossy().into();
                s.s.v_resepti = crate::resepti::Resepti::default();
                s.s.nayte_alku = 600.0;
                s.s.nayte_kesto = 3.0;
                s.aloita();
            }, |s| s.ajo.is_none(), move |_| {
                // lähde on 30 fps, jossa joka 6. kuva on kopio: tulos 25 fps, 3 s = 75 kuvaa
                let t = Tiedot::lue(&k5.join("veljet.mkv").to_string_lossy()).ok();
                let (n, fps) = t.map(|t| (t.n, t.fps_teksti())).unwrap_or((0, String::new()));
                let vali = k5.join("veljet.vaihe1.mkv").exists();
                (n == 75 && fps == "25/1" && !vali, format!("{n} kuvaa @ {fps} fps, välitiedosto jäi: {vali}"))
            }),
            a("näyte ei tallennu asetuksiin", 5, |_| {}, |_| true, |s| {
                let j = serde_json::to_string(&s.s).unwrap_or_default();
                (!j.contains("nayte_"), format!("näyte {} s asetuksissa: {}", s.s.nayte_kesto, j.contains("nayte_")))
            }),
            a("Väritä: esikatselu", 120, |s| { s.s.v_aika = "600".into(); s.v_esikatsele(); }, |s| !s.tyo_kaynnissa,
              |s| (s.v_tex.is_some(), s.v_teksti.clone())),
            a("kuvakaappaus: väritä", 20, |_| {}, kaappaa(&k6, "varita"), |_| (true, String::new())),
            a("Satunnainen esikatselu (2 s) ja soitin", 600, |s| {
                s.tila_valilehti = Valilehti::Varita;
                s.s.esik_kesto = 2.0;
                s.s.esik_vertaa = true;
                s.aloita_esikatselu();
            }, |s| s.ajo.is_none() && s.soitin.as_ref().map(|p| p.tex.is_some()).unwrap_or(false), |s| match &s.soitin {
                Some(p) => {
                    let n = kuvia(&PathBuf::from(&p.ulos));
                    let [w, _] = p.tex.as_ref().unwrap().size();
                    let t = Tiedot::lue_harvennettu(&s.s.v_video).unwrap();
                    let odotettu = t.kehys(2.0);
                    (n == odotettu && w >= 900 && p.alku + p.kesto <= t.kesto,
                     format!("{n}/{odotettu} kuvaa, kohta {}, soittimen kuva {w} px (lähde + tulos)", crate::video::aikateksti(p.alku)))
                }
                None => (false, s.tila.lock().unwrap().0.clone()),
            }),
            a("kuvakaappaus: esikatselusoitin", 20, |_| {}, kaappaa(&kansio, "soitin"), |s| {
                let eri = {
                    // kaksi peräkkäistä esikatselua osuu eri kohtiin
                    let a = crate::gui::satunnainen(0.0, 1000.0);
                    std::thread::sleep(Duration::from_millis(3));
                    let b = crate::gui::satunnainen(0.0, 1000.0);
                    (a - b).abs() > 1e-6
                };
                s.soitin = None;
                (eri, String::new())
            }),
            a("Pysäytä kesken", 120, move |s| {
                // välilehti asetetaan itse, ettei ajo kohdistu edellisen välilehden videoon
                s.valilehti = Valilehti::Varita;
                s.tila_valilehti = Valilehti::Varita;
                s.s.v_video = v2.clone();
                s.s.nayte_kesto = 0.0;
                s.s.v_ulos = s.s.v_ulos.replace("veljet.mkv", "veljet_pysaytys.mkv");
                s.aloita();
            }, {
                let mut t0: Option<Instant> = None;
                move |s| {
                    let t = *t0.get_or_insert_with(Instant::now);
                    if t.elapsed() > Duration::from_secs(10) && s.ajo.is_some() {
                        s.pysayta();
                    }
                    t.elapsed() > Duration::from_secs(10) && s.ajo.is_none()
                }
            }, |s| {
                let t = s.tila.lock().unwrap().0.clone();
                (t.contains("Cancelled"), t)
            }),
            a("Korjaa: väritetty + alkuperäinen, 4 s", 600, move |s| {
                s.valilehti = Valilehti::Korjaa;
                s.tila_valilehti = Valilehti::Korjaa;
                s.s.k_video = format!(r"{D}\5 Presidentin Vävy_bw_final_varitetty.mkv");
                s.s.k_alkuperainen = bw2.clone();
                s.s.k_ulos = s.s.v_ulos.replace("veljet_pysaytys.mkv", "korjattu.mkv");
                s.s.nayte_alku = 30.0;
                s.s.nayte_kesto = 4.0;
                s.aloita();
            }, |s| s.ajo.is_none(), |s| {
                let n = kuvia(&PathBuf::from(&s.s.k_ulos));
                (n == 100, format!("{n} kuvaa"))
            }),
            a("Lisäosat: kaikki asennettu", 30, |s| s.valilehti = Valilehti::Lisaosat, |s| s.ollama.is_some(), |s| {
                let puuttuu: Vec<_> = crate::lisaosat::osat().into_iter().filter(|o| o.asennettu.is_none()).map(|o| o.nimi).collect();
                let nako = s.ollama.as_ref().map(|t| t.nakomallit.len()).unwrap_or(0);
                (puuttuu.is_empty() && !crate::lisaosat::puuttuu_pakollisia() && nako > 0,
                 format!("puuttuu {puuttuu:?}, ffmpeg {:?}, näkömalleja {nako}", crate::lisaosat::ffmpeg_oma()))
            }),
            a("kuvakaappaus: lisaosat", 20, |_| {}, kaappaa(&kansio, "lisaosat"), |_| (true, String::new())),
            a("kuvakaappaus: asetukset", 20, |s| s.valilehti = Valilehti::Asetukset, kaappaa(&kansio, "asetukset"), |_| (true, String::new())),
            a("tauko ennen sulkemista", 5, |_| {}, {
                let mut t0: Option<Instant> = None;
                move |_| t0.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2)
            }, |_| (true, String::new())),
        ];
        Testi { askeleet, i: 0, aloitettu: None, raportti: vec![], kansio, virheita: 0 }
    }

    /// Ajetaan joka kuvassa. Palauttaa true, kun testi on valmis.
    pub fn askel(&mut self, s: &mut Sovellus) -> bool {
        if self.i >= self.askeleet.len() {
            return true;
        }
        let a = &mut self.askeleet[self.i];
        if self.aloitettu.is_none() {
            (a.tee)(s);
            self.aloitettu = Some(Instant::now());
            return false;
        }
        let aikaa = self.aloitettu.unwrap().elapsed();
        let valmis = (a.odota)(s);
        if !valmis && aikaa < Duration::from_secs(a.aikaraja) {
            return false;
        }
        let (ok, tieto) = if valmis { (a.tarkista)(s) } else { (false, "AIKARAJA".to_string()) };
        if !ok {
            self.virheita += 1;
        }
        let rivi = format!("{} {}  ({:.1} s)  {tieto}", if ok { "OK  " } else { "VIKA" }, a.nimi, aikaa.as_secs_f32());
        crate::loki(&format!("[testi] {rivi}"));
        self.raportti.push(rivi);
        self.i += 1;
        self.aloitettu = None;
        let _ = std::fs::write(self.kansio.join("raportti.txt"),
                               format!("{}\nVIRHEITÄ: {}{}\n", self.raportti.join("\n"), self.virheita,
                                       if self.i >= self.askeleet.len() { "\nVALMIS" } else { "" }));
        self.i >= self.askeleet.len()
    }
}
