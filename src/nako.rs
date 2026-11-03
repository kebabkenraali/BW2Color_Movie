//! Näkömalliavustaja (Ollama, paikallinen): suosittelee parhaan väritysehdotuksen A–F ja
//! tarkastaa valmiin videon epäluonnollisten värien varalta (laaturaportti).
//! Näkömalli ei väritä itse – se katsoo kuvia ja vastaa JSON-muodossa.

use std::io::Cursor;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::kuva::Bgr;
use crate::vari::{bgr_lab, kroma};
use crate::video::{Tiedot, aikateksti};
use crate::{edistys, loki};

const OSOITE: &str = "http://127.0.0.1:11434";
const EI_IKKUNAA: u32 = 0x0800_0000;
/// Oletusmalli: qwen2.5vl vertailee useaa kuvaa luotettavasti (qwen3-vl palautti tyhjän vastauksen monen kuvan kysymykseen).
pub const OLETUSMALLI: &str = "qwen2.5vl:7b";

fn oma_palvelin() -> &'static Mutex<Option<Child>> {
    static P: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

fn agentti(aikaraja: u64) -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(aikaraja))).build())
}

pub fn ollama_exe() -> Option<PathBuf> {
    let mut v = Vec::new();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(l).join("Programs").join("Ollama").join("ollama.exe"));
    }
    if let Ok(path) = std::env::var("PATH") {
        v.extend(std::env::split_paths(&path).map(|p| p.join("ollama.exe")));
    }
    v.into_iter().find(|p| p.is_file())
}

pub fn kaynnissa() -> bool {
    agentti(2).get(&format!("{OSOITE}/api/version")).call().is_ok()
}

/// Käynnistää Ollaman taustalle, jos se ei ole jo käynnissä. Sovelluksen käynnistämä palvelin sammutetaan lopuksi.
pub fn varmista_palvelin() -> Result<()> {
    if kaynnissa() {
        return Ok(());
    }
    let exe = ollama_exe().ok_or_else(|| anyhow!("Ollama not found. Install it from ollama.com."))?;
    let lapsi = Command::new(exe)
        .arg("serve")
        .creation_flags(EI_IKKUNAA)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Ollama did not start")?;
    *oma_palvelin().lock().unwrap() = Some(lapsi);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(20) {
        if kaynnissa() {
            loki("Ollama started.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    bail!("Ollama ei vastannut 20 sekunnissa.")
}

/// Pyytää Ollamaa vapauttamaan mallin näytönohjaimen muistista (esim. ennen väritysajoa).
pub fn vapauta_malli(malli: &str) {
    if kaynnissa() {
        let _ = agentti(10).post(&format!("{OSOITE}/api/generate")).send_json(json!({"model": malli, "keep_alive": 0}));
    }
}

pub fn sammuta_oma_palvelin() {
    if let Some(mut c) = oma_palvelin().lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Paikalliset mallit, joilla on kuvankäsittelykyky ("vision").
pub fn nakomallit() -> Result<Vec<String>> {
    varmista_palvelin()?;
    let a = agentti(10);
    let tags: Value = a.get(&format!("{OSOITE}/api/tags")).call()?.body_mut().read_json()?;
    let mut ulos = Vec::new();
    for m in tags["models"].as_array().cloned().unwrap_or_default() {
        let nimi = m["name"].as_str().unwrap_or_default().to_string();
        let tiedot: Value = match a.post(&format!("{OSOITE}/api/show")).send_json(json!({"model": nimi})) {
            Ok(mut r) => r.body_mut().read_json().unwrap_or(Value::Null),
            Err(_) => continue,
        };
        if tiedot["capabilities"].as_array().map(|c| c.iter().any(|x| x == "vision")).unwrap_or(false) {
            ulos.push(nimi);
        }
    }
    ulos.sort();
    Ok(ulos)
}

/// Lataa mallin Ollamaan (/api/pull). `edistys(tila, saatu, koko)`.
pub fn veda_malli(malli: &str, edistys: &mut dyn FnMut(&str, u64, u64), seis: &AtomicBool) -> Result<()> {
    use std::io::BufRead;
    varmista_palvelin()?;
    let vastaus = agentti(24 * 3600)
        .post(&format!("{OSOITE}/api/pull"))
        .send_json(json!({"model": malli, "stream": true}))
        .with_context(|| format!("mallin {malli} lataus ei alkanut"))?;
    let lukija = std::io::BufReader::new(vastaus.into_body().into_reader());
    for rivi in lukija.lines() {
        if seis.load(Ordering::Relaxed) {
            bail!("Lataus keskeytettiin (Ollama jatkaa samasta kohdasta seuraavalla kerralla).");
        }
        let v: Value = serde_json::from_str(&rivi?).unwrap_or(Value::Null);
        if let Some(virhe) = v["error"].as_str() {
            bail!("Ollama: {virhe}");
        }
        edistys(v["status"].as_str().unwrap_or(""), v["completed"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(0));
        if v["status"] == "success" {
            return Ok(());
        }
    }
    bail!("Ollama katkaisi mallin {malli} latauksen.")
}

pub fn poista_malli(malli: &str) -> Result<()> {
    varmista_palvelin()?;
    agentti(30).delete(&format!("{OSOITE}/api/delete")).force_send_body().send_json(json!({"model": malli}))?;
    Ok(())
}

/// Kuva mallille PNG:nä. (JPEG ei kelpaa: image-kirjaston JPEGin Adobe-merkintä saa Ollaman
/// purkajan tulkitsemaan värit väärin – malli näki jokaisessa kuvassa violetin sävyn.)
fn kuva_b64(k: &Bgr, leveys: usize) -> Result<String> {
    let k = if k.w > leveys { k.koon(leveys, (k.h * leveys / k.w).max(2)) } else { k.clone() };
    let mut puskuri = Cursor::new(Vec::new());
    let kuva = image::RgbImage::from_raw(k.w as u32, k.h as u32, k.rgb()).ok_or_else(|| anyhow!("kuva"))?;
    image::DynamicImage::ImageRgb8(kuva).write_to(&mut puskuri, image::ImageFormat::Png)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(puskuri.into_inner()))
}

fn kysy(malli: &str, ohje: &str, kuvat: Vec<String>, skeema: Value) -> Result<Value> {
    varmista_palvelin()?;
    let pyynto = json!({
        "model": malli, "stream": false, "think": false, "format": skeema,
        "options": {"temperature": 0.1},
        "messages": [{"role": "user", "content": ohje, "images": kuvat}],
    });
    // yksi uusi yritys: esim. HTTP 500, kun malli ei heti mahdu näytönohjaimen muistiin
    let mut vastaus = agentti(600).post(&format!("{OSOITE}/api/chat")).send_json(&pyynto);
    if vastaus.is_err() {
        std::thread::sleep(Duration::from_secs(3));
        vastaus = agentti(600).post(&format!("{OSOITE}/api/chat")).send_json(&pyynto);
    }
    let v: Value = vastaus.map_err(|e| anyhow!("Ollama request failed ({malli}): {e}"))?.body_mut().read_json()?;
    let sisalto = v["message"]["content"].as_str().unwrap_or_default().trim().to_string();
    if sisalto.is_empty() {
        bail!("Model {malli} returned an empty answer – try another vision model (default {OLETUSMALLI}).");
    }
    serde_json::from_str(&sisalto).map_err(|e| anyhow!("The model's answer was not valid JSON ({e}): {sisalto}"))
}

// --------------------------------------------------------------------------
// Ehdotusten suositus
// --------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct Suositus {
    pub paras: String,
    pub toinen: String,
    pub perustelu: String,
    pub malli: String,
    pub aika_s: f32,
    /// kohtauksen tyyppi suomeksi (vain kohtausanalyysissa)
    pub kohtaus: String,
}

pub const KOHTAUSTYYPIT: [(&str, &str); 6] = [
    ("day_exterior", "day, exterior"), ("night_exterior", "night, exterior"), ("day_interior", "day, interior"),
    ("night_interior", "evening/night, interior"), ("close_up", "close-up of faces"), ("other", "other"),
];

/// `ehdotukset`: (kirjain, kuvaus, kuva). Palauttaa mallin valitseman parhaan kirjaimen.
pub fn suosittele(malli: &str, ehdotukset: &[(String, String, Bgr)]) -> Result<Suositus> {
    suosittele_vakaasti(malli, ehdotukset, false)
}

/// Vakaa valinta. Näkömalli suosii usein kuvaa sen paikan perusteella (koe: puolet valinnoista vaihtui, kun järjestys
/// käännettiin), joten kysytään kahdessa järjestyksessä. Samat vastaukset = varma. Ristiriidassa ehdokkaat arvioidaan
/// erikseen kalibroidulla virhetarkistuksella; vähiten virheitä voittaa, tasapelissä ensimmäinen ehdotus (DDColor).
pub fn suosittele_vakaasti(malli: &str, ehdotukset: &[(String, String, Bgr)], kohtaus: bool) -> Result<Suositus> {
    let t0 = Instant::now();
    let mut eteen = suosittele_kohtaus(malli, ehdotukset, kohtaus)?;
    let kaannetty: Vec<(String, String, Bgr)> = ehdotukset.iter().rev().cloned().collect();
    let taakse = suosittele_kohtaus(malli, &kaannetty, false)?;
    if eteen.paras == taakse.paras {
        eteen.perustelu = format!("(certain) {}", eteen.perustelu);
        eteen.aika_s = t0.elapsed().as_secs_f32();
        return Ok(eteen);
    }
    // ristiriita: ehdokkaiden virhepisteet erikseen
    let mut parhaat: Vec<(u32, usize, String)> = Vec::new();
    for (i, (k, _, kuva)) in ehdotukset.iter().enumerate() {
        if *k == eteen.paras || *k == taakse.paras {
            parhaat.push((virhepisteet(malli, kuva)?, i, k.clone()));
        }
    }
    parhaat.sort();
    let (pisteet, _, voittaja) = parhaat[0].clone();
    let toinen = if voittaja == eteen.paras { taakse.paras.clone() } else { eteen.paras.clone() };
    eteen.perustelu = format!("(uncertain: {} or {}; fewest errors {voittaja}, {pisteet} pts) {}",
                              eteen.paras, taakse.paras, eteen.perustelu);
    eteen.paras = voittaja;
    eteen.toinen = toinen;
    eteen.aika_s = t0.elapsed().as_secs_f32();
    Ok(eteen)
}

/// Kalibroitu virhetarkistus yhdelle kuvalle: väärä iho 3, harmaa ruutu 3, harmaa läikkä 1, värisävy 1 pistettä.
fn virhepisteet(malli: &str, k: &Bgr) -> Result<u32> {
    let skeema = json!({"type": "object", "properties": {
        "skin_wrong": {"type": "boolean"}, "grey_patch": {"type": "boolean"}, "whole_grey": {"type": "boolean"},
        "color_cast": {"type": "boolean"}, "note": {"type": "string"}},
        "required": ["skin_wrong", "grey_patch", "whole_grey", "color_cast", "note"]});
    let v = kysy(malli, TARKISTUSLISTA, vec![kuva_b64(k, 640)?], skeema)?;
    let kylla = |x: &str| v[x].as_bool().unwrap_or(false) as u32;
    Ok(3 * kylla("skin_wrong") + 3 * kylla("whole_grey") + kylla("grey_patch") + kylla("color_cast"))
}

/// Tarkat kyllä/ei-kysymykset (kalibroitu tunnetuilla hyvillä/huonoilla kuvilla).
const TARKISTUSLISTA: &str = "Look at this colorized film frame and answer each question strictly with true or false. \
    skin_wrong: is any visible human skin clearly an impossible color (green, blue, purple, grey)? \
    grey_patch: is there a clearly grey, uncolored patch inside an otherwise colored image? \
    whole_grey: is the whole image essentially black-and-white? \
    color_cast: is the whole image covered by one strong unnatural tint (e.g. purple, green)? \
    Answer false when unsure. JSON with these four booleans and note (short English).";

/// Kuten `suosittele`; `kohtaus` = true pyytää lisäksi kohtauksen tyypin (kysytään ensin, jotta malli
/// katsoo kohtausta ennen valintaa – Ollama täyttää kentät skeeman järjestyksessä).
pub fn suosittele_kohtaus(malli: &str, ehdotukset: &[(String, String, Bgr)], kohtaus: bool) -> Result<Suositus> {
    if ehdotukset.len() < 2 {
        bail!("At least two suggestions are needed for a comparison.");
    }
    let t0 = Instant::now();
    let kirjaimet: Vec<String> = ehdotukset.iter().map(|e| e.0.clone()).collect();
    let luettelo: Vec<String> = ehdotukset.iter().enumerate().map(|(i, e)| format!("Image {} = {} ({})", i + 1, e.0, e.1)).collect();
    let ohje = format!(
        "You are an expert film colorist. The {} images are colorization candidates of the SAME black-and-white \
         film frame (Finnish feature film, around the 1970s). {}.\n\
         Pick the candidate that looks most natural and historically plausible. Judge: realistic skin tones (no grey, \
         blue, green or orange skin), no color bleeding across object edges, no strong unnatural color cast, plausible \
         clothing / background / vehicle colors, enough color but not oversaturated. \
         Answer in JSON: best = letter, second = letter, reason = one short sentence in English.",
        ehdotukset.len(), luettelo.join("; "));
    let kuvat = ehdotukset.iter().map(|e| kuva_b64(&e.2, 448)).collect::<Result<Vec<_>>>()?;
    let mut ominaisuudet = serde_json::Map::new();
    let mut pakolliset = Vec::new();
    let ohje = if kohtaus {
        let tyypit: Vec<&str> = KOHTAUSTYYPIT.iter().map(|t| t.0).collect();
        ominaisuudet.insert("scene".into(), json!({"type": "string", "enum": tyypit}));
        pakolliset.push("scene");
        format!("{ohje}\nFirst classify the scene as scene = one of {}.", tyypit.join(", "))
    } else {
        ohje
    };
    ominaisuudet.insert("best".into(), json!({"type": "string", "enum": kirjaimet}));
    ominaisuudet.insert("second".into(), json!({"type": "string", "enum": kirjaimet}));
    ominaisuudet.insert("reason".into(), json!({"type": "string"}));
    pakolliset.extend(["best", "second", "reason"]);
    let skeema = json!({"type": "object", "properties": ominaisuudet, "required": pakolliset});
    let v = kysy(malli, &ohje, kuvat, skeema)?;
    let paras = v["best"].as_str().unwrap_or_default().to_string();
    if !kirjaimet.contains(&paras) {
        bail!("Malli vastasi tuntemattoman ehdotuksen: {paras}");
    }
    Ok(Suositus {
        paras,
        toinen: v["second"].as_str().unwrap_or_default().to_string(),
        perustelu: v["reason"].as_str().unwrap_or_default().to_string(),
        malli: malli.to_string(),
        aika_s: t0.elapsed().as_secs_f32(),
        kohtaus: v["scene"].as_str().and_then(|s| KOHTAUSTYYPIT.iter().find(|t| t.0 == s)).map(|t| t.1.to_string()).unwrap_or_default(),
    })
}

// --------------------------------------------------------------------------
// Laaturaportti
// --------------------------------------------------------------------------
fn ongelman_nimi(o: &str) -> &str {
    match o {
        "iho" => "unnatural skin",
        "vuoto" => "colour bleeding over edges",
        "harmaa_alue" => "uncoloured area",
        "savy" => "strong colour cast",
        "ylikyllainen" => "oversaturated",
        _ => "other",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Havainto {
    pub kehys: usize,
    pub aika: String,
    pub kunnossa: bool,
    pub vakavuus: u8,
    pub ongelmat: Vec<String>,
    pub kuvaus: String,
    pub kroma: f32,
    pub harmaa_osuus: f32,
    pub kuva: String,
}

pub struct Raportti {
    pub html: PathBuf,
    pub havainnot: Vec<Havainto>,
}

struct Mittarit {
    kroma: f32,
    kroma_p99: f32,
    harmaa: f32,
    laikku: bool,
    a_ka: f32,
    b_ka: f32,
    /// koko kuvan voimakas epäluonnollinen sävy (violetti, vihreä, sininen)
    savy: bool,
}

/// Mitatut arvot: keskikroma, kylläisimmät värit, harmaan osuus ja värittämättömät läikät.
fn mittarit(k: &Bgr) -> Mittarit {
    let lab = bgr_lab(k);
    let c = kroma(&lab.ab);
    let (mut s, mut n, mut harmaa, mut sa, mut sb) = (0.0f64, 0usize, 0usize, 0.0f64, 0.0f64);
    for (i, &l) in lab.l.data.iter().enumerate() {
        if l > 15.0 && l < 92.0 {
            s += c.data[i] as f64;
            sa += lab.ab.x.data[i] as f64;
            sb += lab.ab.y.data[i] as f64;
            n += 1;
            if c.data[i] < 3.0 {
                harmaa += 1;
            }
        }
    }
    let (a_ka, b_ka) = if n > 0 { ((sa / n as f64) as f32, (sb / n as f64) as f32) } else { (0.0, 0.0) };
    // lämmin keltainen/oranssi (b > 0) on vanhoissa väritetyissä elokuvissa tavallinen eikä ole virhe
    let savy = (a_ka > 10.0 && b_ka < 5.0) || a_ka < -8.0 || b_ka < -10.0 || a_ka.hypot(b_ka) > 40.0;
    let mut lajit = c.data.clone();
    lajit.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p99 = lajit.get(lajit.len() * 99 / 100).copied().unwrap_or(0.0);
    let kroma_ka = if n > 0 { (s / n as f64) as f32 } else { 0.0 };
    Mittarit {
        kroma: kroma_ka,
        kroma_p99: p99,
        harmaa: if n > 0 { harmaa as f32 / n as f32 } else { 0.0 },
        laikku: kroma_ka >= 4.0 && crate::tuotanto::laikut(&lab.l, &c).is_some(),
        a_ka,
        b_ka,
        savy,
    }
}

pub fn laaturaportti(video: &str, naytteita: usize, malli: &str, seis: &AtomicBool) -> Result<Raportti> {
    let t = Tiedot::lue(video)?;
    let p = Path::new(video);
    let kansio = p.with_file_name(format!("{}_quality_report", p.file_stem().unwrap_or_default().to_string_lossy()));
    std::fs::create_dir_all(&kansio)?;
    let n = naytteita.clamp(1, t.n);
    let kehykset: Vec<usize> = (0..n).map(|i| ((i as f64 + 0.5) * t.n as f64 / n as f64) as usize).collect();
    // Kalibroitu tunnetuilla hyvillä/huonoilla kuvilla: tarkat kyllä/ei-kysymykset erottelevat luotettavasti,
    // yleinen "arvioi 1–10" tai "etsi virheet" -kysymys merkitsee lähes kaiken virheelliseksi.
    let ohje = TARKISTUSLISTA;
    let skeema = json!({"type": "object", "properties": {
        "skin_wrong": {"type": "boolean"}, "grey_patch": {"type": "boolean"}, "whole_grey": {"type": "boolean"},
        "color_cast": {"type": "boolean"}, "note": {"type": "string"}},
        "required": ["skin_wrong", "grey_patch", "whole_grey", "color_cast", "note"]});
    let (w, h) = t.koko_enintaan(640);
    let mut havainnot = Vec::new();
    let t0 = Instant::now();
    loki(&format!("Quality report: {n} samples from {} with model {malli}", p.file_name().unwrap_or_default().to_string_lossy()));
    for (i, &f) in kehykset.iter().enumerate() {
        if seis.load(Ordering::Relaxed) {
            bail!("Cancelled.");
        }
        let k = crate::video::hae_kehys(&t, f, Some((w, h)))?;
        let m = mittarit(&k);
        let v = kysy(malli, ohje, vec![kuva_b64(&k, 640)?], skeema.clone())?;
        let kylla = |x: &str| v[x].as_bool().unwrap_or(false);
        let mut ongelmat = Vec::new();
        let mut vakavuus = 0u8;
        let mut huomiot = Vec::new();
        let mut lisaa = |o: &str, vak: u8, h: String| {
            if !ongelmat.iter().any(|x: &String| x == o) {
                ongelmat.push(o.to_string());
            }
            vakavuus = vakavuus.max(vak);
            if !h.is_empty() {
                huomiot.push(h);
            }
        };
        // näkömallin havainnot
        if kylla("skin_wrong") {
            lisaa("iho", 3, String::new());
        }
        if kylla("color_cast") {
            // pelkkä mallin arvio on vain "tarkista"; mitattu sävy nostaa ongelmaksi
            if m.savy {
                lisaa("savy", 2, format!("measured colour cast (a {:.0}, b {:.0})", m.a_ka, m.b_ka));
            } else {
                lisaa("savy", 1, String::new());
            }
        }
        if kylla("grey_patch") && !m.laikku {
            // malli sekoittaa joskus harmaan seinän värittämättömään alueeseen: ilman mittausta vain "tarkista"
            lisaa("harmaa_alue", 1, String::new());
        }
        if kylla("whole_grey") {
            lisaa("harmaa_alue", 3, String::new());
        }
        // mitatut havainnot täydentävät mallia
        if m.kroma < 4.0 {
            lisaa("harmaa_alue", 3, format!("frame almost uncoloured (mean chroma {:.1})", m.kroma));
        } else if m.laikku {
            lisaa("harmaa_alue", 2, "uncoloured patch (measured)".into());
        }
        if m.kroma_p99 > 85.0 {
            lisaa("ylikyllainen", 1, format!("very saturated colours (chroma 99th pct = {:.0})", m.kroma_p99));
        }
        let malli_huomio = v["note"].as_str().unwrap_or_default().trim().to_string();
        if !ongelmat.is_empty() && !malli_huomio.is_empty() {
            huomiot.push(format!("model: {malli_huomio}"));
        }
        let kuvaus = huomiot.join("; ");
        let (kr, harmaa) = (m.kroma, m.harmaa);
        let kunnossa = vakavuus < 2;
        let kuva = format!("k_{f:07}.jpg");
        image::save_buffer(kansio.join(&kuva), &k.rgb(), k.w as u32, k.h as u32, image::ExtendedColorType::Rgb8)?;
        havainnot.push(Havainto { kehys: f, aika: aikateksti(t.aika(f)), kunnossa, vakavuus, ongelmat, kuvaus, kroma: kr, harmaa_osuus: harmaa, kuva });
        let nopeus = (i + 1) as f64 / t0.elapsed().as_secs_f64().max(1e-6);
        edistys(&format!("Quality report: {}/{n} frames ({:.0} %)  remaining ~{}", i + 1, 100.0 * (i + 1) as f64 / n as f64,
                         &aikateksti((n - i - 1) as f64 / nopeus)[..7]), i + 1, n);
    }
    let html = kirjoita_html(&kansio, video, malli, &havainnot)?;
    std::fs::write(kansio.join("report.json"), serde_json::to_string_pretty(&havainnot)?)?;
    let ongelmia = havainnot.iter().filter(|h| !h.kunnossa).count();
    loki(&format!("Quality report done: problems in {ongelmia}/{n} samples. {}", html.display()));
    Ok(Raportti { html, havainnot })
}

fn html_suojaa(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn kirjoita_html(kansio: &Path, video: &str, malli: &str, h: &[Havainto]) -> Result<PathBuf> {
    let ongelmat: Vec<&Havainto> = h.iter().filter(|x| !x.kunnossa).collect();
    let tarkista = h.iter().filter(|x| x.kunnossa && x.vakavuus == 1).count();
    let mut rivit = String::new();
    let mut lajiteltu: Vec<&Havainto> = h.iter().filter(|x| x.vakavuus >= 1).collect();
    lajiteltu.sort_by(|a, b| b.vakavuus.cmp(&a.vakavuus).then(a.kehys.cmp(&b.kehys)));
    for x in &lajiteltu {
        let nimet: Vec<&str> = x.ongelmat.iter().map(|o| ongelman_nimi(o)).collect();
        let taso = if x.vakavuus == 1 { "check".to_string() } else { "●".repeat(x.vakavuus as usize) };
        rivit += &format!(
            "<tr class=\"v{}\"><td><img src=\"{}\" loading=\"lazy\"></td><td><b>{}</b><br>frame {}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.1} / {:.0} %</td></tr>\n",
            x.vakavuus, x.kuva, x.aika, x.kehys, taso, html_suojaa(&nimet.join(", ")),
            html_suojaa(&x.kuvaus), x.kroma, 100.0 * x.harmaa_osuus);
    }
    let kaikki: String = h.iter().map(|x| format!("<a href=\"{}\" title=\"{} {}\" class=\"{}\"><img src=\"{}\" loading=\"lazy\"></a>",
                                                   x.kuva, x.aika, html_suojaa(&x.kuvaus), if x.kunnossa { "ok" } else { "fault" }, x.kuva)).collect();
    let sivu = format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Quality report</title><style>
:root{{color-scheme:light dark;--tausta:#fafafa;--teksti:#222;--reuna:#ddd;--v1:#e8b400;--v2:#e07020;--v3:#d02020}}
@media (prefers-color-scheme:dark){{:root{{--tausta:#1c1c1c;--teksti:#eee;--reuna:#444}}}}
body{{font-family:Segoe UI,system-ui,sans-serif;background:var(--tausta);color:var(--teksti);margin:0 16px 40px;max-width:1300px}}
h1{{font-size:1.4em}} table{{border-collapse:collapse;width:100%}} td,th{{border-bottom:1px solid var(--reuna);padding:6px;text-align:left;vertical-align:top}}
td img{{width:320px;border-radius:4px}} tr.v1 td:nth-child(3){{color:var(--v1)}} tr.v2 td:nth-child(3){{color:var(--v2)}} tr.v3 td:nth-child(3){{color:var(--v3)}}
.kaikki img{{width:150px;margin:2px;border:3px solid transparent;border-radius:3px}} .kaikki a.fault img{{border-color:var(--v2)}}
.yht{{opacity:.75}}</style></head><body>
<h1>Quality report</h1><p class="yht">{}<br>Vision model {} · {} samples · problems in <b>{}</b> samples · {} to check.
The rating combines the vision model's answers with measured values (grey frames, patches, colour cast); check the marked spots by eye.</p>
<h2>Findings (most severe first)</h2>
<table><tr><th>Image</th><th>Position</th><th>Severity</th><th>Problem</th><th>Description</th><th>Chroma / grey</th></tr>
{}</table><h2>All samples</h2><div class="kaikki">{}</div></body></html>"#,
        html_suojaa(video), html_suojaa(malli), h.len(), ongelmat.len(), tarkista, if rivit.is_empty() { "<tr><td colspan=6>No problems found.</td></tr>".into() } else { rivit }, kaikki);
    let polku = kansio.join("report.html");
    std::fs::write(&polku, sivu)?;
    Ok(polku)
}
