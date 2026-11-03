//! BW2Color Movie: mustavalkoelokuvan väritys natiivina Windows-sovelluksena (käyttöliittymä englanniksi).
//! Ilman argumentteja avautuu käyttöliittymä; komentorivi:  BW2Color.exe {colorize|color|repair|exemplar|models|install} ...

#![windows_subsystem = "windows"]

mod analyysi;
mod gui;
mod kohtausmuisti;
mod kuva;
mod lisaosat;
mod mallit;
mod nako;
mod piirteet;
mod putki;
mod resepti;
mod tunnistin;
mod tuotanto;
mod vakautus;
mod vari;
mod video;
mod virtaus;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};

pub enum Viesti {
    Loki(String),
    Tila(String, usize, usize),
}

type Vastaanottaja = Box<dyn Fn(Viesti) + Send + Sync>;

fn vastaanottaja() -> &'static Mutex<Option<Vastaanottaja>> {
    static V: OnceLock<Mutex<Option<Vastaanottaja>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(None))
}

pub fn aseta_vastaanottaja(f: Vastaanottaja) {
    *vastaanottaja().lock().unwrap() = Some(f);
}

pub fn loki(t: &str) {
    match &*vastaanottaja().lock().unwrap() {
        Some(f) => f(Viesti::Loki(t.to_string())),
        None => println!("{t}"),
    }
}

pub fn edistys(t: &str, i: usize, n: usize) {
    match &*vastaanottaja().lock().unwrap() {
        Some(f) => f(Viesti::Tila(t.to_string(), i, n)),
        None => {
            use std::io::Write;
            print!("\r{t}   ");
            let _ = std::io::stdout().flush();
            if i >= n {
                println!();
            }
        }
    }
}

// --------------------------------------------------------------------------
// Komentorivi
// --------------------------------------------------------------------------
struct Argit(Vec<String>);

impl Argit {
    fn arvo(&mut self, nimi: &str) -> Option<String> {
        let i = self.0.iter().position(|a| a == nimi)?;
        if i + 1 >= self.0.len() {
            return None;
        }
        let v = self.0.remove(i + 1);
        self.0.remove(i);
        Some(v)
    }
    fn lippu(&mut self, nimi: &str) -> bool {
        match self.0.iter().position(|a| a == nimi) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }
    fn luku<T: std::str::FromStr>(&mut self, nimi: &str, oletus: T) -> Result<T> {
        match self.arvo(nimi) {
            Some(v) => v.parse().map_err(|_| anyhow!("invalid value {nimi} {v}")),
            None => Ok(oletus),
        }
    }
}

const OHJE: &str = "Usage:
  BW2Color.exe                       graphical user interface
  BW2Color.exe colorize VIDEO [--model M] [--fill-model M] [--grey-threshold 4]   RECOMMENDED: colorize -> check -> repair
                                     (patches and grey frames are filled, colours compared with previous frames)
  BW2Color.exe check VIDEO           frame-by-frame check of a colorized video (grey frames, patches, colour jumps)
  BW2Color.exe color VIDEO [--model M] [--reference IMAGE] [--ref-strength 0.7] [--h 0] [--s 1] [--v 1]
                                     colorize only (no check/repair)
  BW2Color.exe repair VIDEO [--original BW-VIDEO] [--no-fill] [--fill-model M] [--grey-threshold 4]
  BW2Color.exe exemplar PROJECT.json
  BW2Color.exe models | install ffmpeg|dinov2_small|vision:OLLAMA-MODEL   (install add-ons from the command line)
  BW2Color.exe quality VIDEO [--samples 40] [--vision-model qwen2.5vl:7b]   quality report with a vision model (Ollama)
  BW2Color.exe analyse PROJECT.json [--min-duration 1] [--max N] [--ddcolor M]   scene analysis for an Exemplar project
  BW2Color.exe vision-models                                            local Ollama vision models
Common: --out FILE --device auto|gpu|cpu --work-width 960 --window 7 --no-flow --smoothing 1 --memory 0.98 --no-scene-memory
        --lock 0.6 --lock-classes skin,sky,vegetation --cut-threshold 0.35 --start S --duration S
        --codec x264|x265|nvenc --crf 14 --audio FILE
colorize/color: --keep-duplicates   do not drop duplicate frames (default: e.g. 30 fps -> true 25 fps)
Film styles of the fine-tuned model: --model ddcolor_suomi | ddcolor_suomi_technicolor | ddcolor_suomi_eastmancolor
        | ddcolor_suomi_kodachrome | ddcolor_suomi_moderni";

/// Englanninkieliset komennot ja valitsimet sisäisiksi (suomenkieliset toimivat edelleen).
const KOMENNOT: &[(&str, &str)] = &[
    ("colorize", "putki"), ("check", "tarkista"), ("color", "vari"), ("repair", "korjaa"), ("models", "mallit"),
    ("install", "lataa"), ("quality", "laatu"), ("analyse", "analysoi"), ("analyze", "analysoi"), ("vision-models", "nakomallit"),
    ("help", "ohje"),
];
const VALITSIMET: &[(&str, &str)] = &[
    ("--model", "--malli"), ("--fill-model", "--tayta-malli"), ("--grey-threshold", "--harmaa-raja"), ("--gray-threshold", "--harmaa-raja"),
    ("--reference", "--referenssi"), ("--ref-strength", "--ref-voima"), ("--original", "--alkuperainen"), ("--no-fill", "--ei-taytto"),
    ("--samples", "--naytteita"), ("--vision-model", "--nako-malli"), ("--min-duration", "--min-kesto"), ("--max", "--enintaan"),
    ("--out", "--ulos"), ("--device", "--laite"), ("--work-width", "--tyoleveys"), ("--window", "--ikkuna"), ("--no-flow", "--ei-virtausta"),
    ("--smoothing", "--tasaus-voima"), ("--memory", "--muisti"), ("--no-scene-memory", "--ei-kohtausmuistia"), ("--lock", "--lukitus"),
    ("--lock-classes", "--lukitus-luokat"), ("--cut-threshold", "--leikkaus-raja"), ("--start", "--alku"), ("--duration", "--kesto"),
    ("--codec", "--koodekki"), ("--audio", "--aanilahde"), ("--keep-duplicates", "--sailyta-kopiot"),
];

fn sisaiseksi(mut argit: Vec<String>) -> Vec<String> {
    if let Some((_, k)) = argit.first().and_then(|a| KOMENNOT.iter().find(|(e, _)| e == a)) {
        argit[0] = k.to_string();
    }
    for a in argit.iter_mut().skip(1) {
        if let Some((_, s)) = VALITSIMET.iter().find(|(e, _)| e == a) {
            *a = s.to_string();
        } else if let Some(m) = a.strip_prefix("vision:") {
            *a = format!("nako:{m}");
        }
    }
    // lukitusluokat englanniksi
    if let Some(i) = argit.iter().position(|a| a == "--lukitus-luokat") {
        if let Some(v) = argit.get_mut(i + 1) {
            *v = v.replace("skin", "iho").replace("sky", "taivas").replace("vegetation", "kasvit");
        }
    }
    argit
}

fn komentorivi(argit: Vec<String>) -> Result<()> {
    let mut a = Argit(sisaiseksi(argit));
    let komento = a.0.remove(0);
    match komento.as_str() {
        "mallit" => {
            for m in mallit::MALLIT {
                println!("{:22} {:9} {}", m.nimi, if mallit::on_ladattu(m.nimi) { "installed" } else { "missing" }, m.kuvaus);
            }
            for (n, k) in mallit::TYYLIT {
                println!("{:22} {:9} {}", n, if mallit::on_ladattu(n) { "installed" } else { "missing" }, k);
            }
            return Ok(());
        }
        "lataa" => {
            for nimi in a.0.clone() {
                let seis = AtomicBool::new(false);
                let mut e = |s: u64, k: u64| edistys(&format!("{nimi}: {} / {} MB", s >> 20, k >> 20), s as usize, k.max(1) as usize);
                if nimi == "ffmpeg" {
                    lisaosat::asenna_ffmpeg(&mut e, &seis)?;
                } else if let Some(m) = nimi.strip_prefix("nako:") {
                    nako::veda_malli(m, &mut |t, s, k| edistys(&format!("{m}: {t}"), s as usize, k.max(1) as usize), &seis)?;
                } else if nimi == piirteet::NIMI {
                    piirteet::lataa(&mut e, &seis)?;
                } else {
                    mallit::lataa(&nimi, &mut e, &seis)?;
                }
                println!("\n{nimi}: installed");
            }
            return Ok(());
        }
        "nakomallit" => {
            for m in nako::nakomallit()? {
                println!("{m}");
            }
            nako::sammuta_oma_palvelin();
            return Ok(());
        }
        "analysoi" => {
            let projekti = a.0.first().cloned().ok_or_else(|| anyhow!("project missing\n{OHJE}"))?;
            let asetukset = analyysi::AnalyysiAsetukset {
                leikkaus_raja: a.luku("--leikkaus-raja", 0.35)?,
                ddcolor: a.arvo("--ddcolor").unwrap_or_else(|| "ddcolor_suomi".into()),
                laite: a.arvo("--laite").unwrap_or_else(|| "auto".into()),
                nako_malli: a.arvo("--nako-malli").unwrap_or_else(|| nako::OLETUSMALLI.into()),
                min_kesto_s: a.luku("--min-kesto", 1.0)?,
                enintaan: a.luku("--enintaan", 0usize)?,
            };
            let mut p = resepti::Projekti::lue(&projekti)?;
            let alkup = p.clone();
            let tulos = analyysi::kohtausanalyysi(&alkup, &analyysi::kuvakansio(&projekti), &asetukset, &AtomicBool::new(false),
                                                  &mut |kp| {
                p.kiinnipisteet.retain(|k| k.kehys != kp.kehys);
                p.kiinnipisteet.push(kp);
                p.kiinnipisteet.sort_by_key(|k| k.kehys);
                if let Err(e) = p.tallenna(&projekti) {
                    loki(&format!("Saving the project failed: {e}"));
                }
            });
            nako::sammuta_oma_palvelin();
            let y = tulos?;
            println!("Analysed {} / {} scenes, errors {}. Project: {projekti}", y.analysoitu, y.kohtauksia, y.virheita);
            return Ok(());
        }
        "laatu" => {
            let n = a.luku("--naytteita", 40usize)?;
            let malli = a.arvo("--nako-malli").unwrap_or_else(|| nako::OLETUSMALLI.into());
            let video = a.0.first().cloned().ok_or_else(|| anyhow!("video missing\n{OHJE}"))?;
            let tulos = nako::laaturaportti(&video, n, &malli, &AtomicBool::new(false));
            nako::sammuta_oma_palvelin();
            let r = tulos?;
            for h in r.havainnot.iter().filter(|h| !h.kunnossa) {
                println!("  {}  severity {}  {}  {}", h.aika, h.vakavuus, h.ongelmat.join(","), h.kuvaus);
            }
            println!("Report: {}", r.html.display());
            return Ok(());
        }
        "vari" | "korjaa" | "exemplar" | "putki" | "tarkista" => {}
        "-h" | "--help" | "ohje" => {
            println!("{OHJE}");
            return Ok(());
        }
        k => bail!("Unknown command {k}\n{OHJE}"),
    }
    let mut s = tuotanto::Asetukset { tila: komento.clone(), ..Default::default() };
    s.ulos = a.arvo("--ulos").unwrap_or_default();
    s.laite = a.arvo("--laite").unwrap_or_else(|| "auto".into());
    s.tyoleveys = a.luku("--tyoleveys", 960)?;
    s.ikkuna = a.luku("--ikkuna", 7)?;
    s.virtaus = !a.lippu("--ei-virtausta");
    s.tasaus_voima = a.luku("--tasaus-voima", 1.0)?;
    s.muisti = a.luku("--muisti", 0.98)?;
    s.kohtausmuisti = !a.lippu("--ei-kohtausmuistia");
    s.lukitus = a.luku("--lukitus", 0.6)?;
    if let Some(l) = a.arvo("--lukitus-luokat") {
        s.lukitus_luokat = l.split(',').filter(|x| !x.is_empty()).map(String::from).collect();
    }
    s.leikkaus_raja = a.luku("--leikkaus-raja", 0.35)?;
    s.alku_s = a.luku("--alku", 0.0)?;
    s.kesto_s = a.luku("--kesto", 0.0)?;
    s.koodekki = a.arvo("--koodekki").unwrap_or_else(|| "x264".into());
    s.crf = a.luku("--crf", 14)?;
    s.aanilahde = a.arvo("--aanilahde").unwrap_or_default();
    match komento.as_str() {
        "vari" | "putki" => {
            s.tayta_malli = a.arvo("--tayta-malli").unwrap_or_else(|| "ddcolor_suomi_technicolor".into());
            s.harmaa_raja = a.luku("--harmaa-raja", 4.0)?;
            s.poista_kopiot = !a.lippu("--sailyta-kopiot");
            s.resepti = resepti::Resepti {
                menetelma: a.arvo("--malli").unwrap_or_else(|| "ddcolor_suomi".into()),
                referenssi: a.arvo("--referenssi").unwrap_or_default(),
                ref_voima: a.luku("--ref-voima", 0.7)?,
                h: a.luku("--h", 0.0)?,
                s: a.luku("--s", 1.0)?,
                v: a.luku("--v", 1.0)?,
                nimi: String::new(),
            };
        }
        "korjaa" => {
            s.alkuperainen = a.arvo("--alkuperainen").unwrap_or_default();
            s.tayta_harmaat = !a.lippu("--ei-taytto");
            s.tayta_malli = a.arvo("--tayta-malli").unwrap_or_else(|| "ddcolor_suomi_technicolor".into());
            s.harmaa_raja = a.luku("--harmaa-raja", 4.0)?;
        }
        _ => {
            s.projekti = String::new();
        }
    }
    let kohde = a.0.first().cloned().ok_or_else(|| anyhow!("video/project missing\n{OHJE}"))?;
    if komento == "exemplar" {
        s.projekti = kohde;
    } else {
        s.video = kohde;
    }
    if komento == "tarkista" {
        let t = putki::tarkista_video(&s.video, s.tyoleveys, a.luku("--harmaa-raja", 4.0)?, s.leikkaus_raja, &AtomicBool::new(false))?;
        println!("problem frames {} / {}", t.ongelmia(), t.kuvia);
        let (ka, p95, n) = putki::valkyntamittari(&s.video, s.leikkaus_raja, &AtomicBool::new(false))?;
        println!("colour flicker within scenes: mean {ka:.2}, 95th percentile {p95:.2} (Lab, {n} frame pairs)");
        return Ok(());
    }
    tuotanto::suorita(s, Arc::new(AtomicBool::new(false)))?;
    Ok(())
}

fn main() {
    let argit: Vec<String> = std::env::args().skip(1).collect();
    if argit.is_empty() {
        if let Err(e) = gui::kaynnista() {
            rfd::MessageDialog::new().set_title("BW2Color Movie").set_description(format!("Start-up failed:\n{e}")).show();
        }
        return;
    }
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    if let Err(e) = komentorivi(argit) {
        eprintln!("\nERROR: {e:#}");
        std::process::exit(1);
    }
}
