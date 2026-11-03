//! Videon luku ja kirjoitus ffmpegillä (kuvatarkka haku, ääkköspolut, ääni lähteestä).

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::kuva::Bgr;

const EI_IKKUNAA: u32 = 0x0800_0000;

/// Sovelluksen kotikansio (all-in-one): exe:n kansio, jossa ovat models\, ffmpeg\bin\, asetukset ja
/// esikatselut. Kehitysajossa target-kansiosta haetaan yläkansioista `models` tai `Varita\models`.
pub fn sovelluskansio() -> PathBuf {
    static K: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
        // asennettuna sovelluskansio on aina exen kansio (myös tyhjänä ennen lisäosia)
        if !exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target")) {
            return exe;
        }
        let mut k = exe.clone();
        for _ in 0..5 {
            if k.join("models").is_dir() {
                return k;
            }
            // kehitysajo (natiivi\target\release): sovelluskansio on rinnalla oleva Varita\
            if k.join("Varita").join("models").is_dir() {
                return k.join("Varita");
            }
            match k.parent() {
                Some(p) => k = p.to_path_buf(),
                None => break,
            }
        }
        exe
    })
    .clone()
}

/// Etsii ohjelman: <sovellus>\ffmpeg\bin, <sovellus>\ffmpeg, <sovellus>, sitten PATH.
pub fn ohjelma(nimi: &str) -> Result<PathBuf> {
    let k = sovelluskansio();
    let tiedosto = format!("{nimi}.exe");
    for p in [k.join("ffmpeg").join("bin"), k.join("ffmpeg"), k.clone(), k.join("..").join("ffmpeg").join("bin")] {
        let c = p.join(&tiedosto);
        if c.is_file() {
            return Ok(c);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for p in std::env::split_paths(&path) {
            let c = p.join(&tiedosto);
            if c.is_file() {
                return Ok(c);
            }
        }
    }
    bail!("{tiedosto} not found. Install FFmpeg on the Add-ons tab or copy ffmpeg.exe and ffprobe.exe into {}", k.join("ffmpeg").join("bin").display())
}

fn komento(nimi: &str) -> Result<Command> {
    let mut c = Command::new(ohjelma(nimi)?);
    c.creation_flags(EI_IKKUNAA);
    Ok(c)
}

#[derive(Clone, Debug)]
pub struct Tiedot {
    pub polku: String,
    pub w: usize,
    pub h: usize,
    pub fps_osoittaja: u64,
    pub fps_nimittaja: u64,
    pub fps: f64,
    pub n: usize,
    pub kesto: f64,
    pub aanta: bool,
    pub matriisi: String,
    /// lähde on vaihtuvanopeuksinen; käsitellään kiinteällä nopeudella `fps`
    pub vfr: bool,
    /// lähteen todellinen keskimääräinen kuvanopeus
    pub keski_fps: f64,
    /// Kopioruutujen poisto: joka N:s kuva on kopio (esim. 25 -> 30 fps -muunnos, N = 6) ja se pudotetaan
    /// lukiessa; tällöin `fps` on todellinen nopeus ja `lahde_fps` tiedoston nimellinen. 0 = ei poistoa.
    pub harvennus: usize,
    pub lahde_fps: (u64, u64),
}

fn syt(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { syt(b, a % b) }
}

fn murto(s: &str) -> (u64, u64) {
    let mut o = s.split('/');
    let a = o.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let b = o.next().and_then(|x| x.parse().ok()).unwrap_or(1);
    (a, b.max(1))
}

impl Tiedot {
    pub fn lue(polku: &str) -> Result<Tiedot> {
        if !Path::new(polku).is_file() {
            bail!("File not found: {polku}");
        }
        let o = komento("ffprobe")?
            .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
            .arg(polku)
            .output()
            .context("ffprobe did not start")?;
        if !o.status.success() {
            bail!("Cannot read the video: {polku}\n{}", String::from_utf8_lossy(&o.stderr));
        }
        let j: serde_json::Value = serde_json::from_slice(&o.stdout)?;
        let virrat = j["streams"].as_array().cloned().unwrap_or_default();
        let v = virrat.iter().find(|s| s["codec_type"] == "video").ok_or_else(|| anyhow!("The file has no video stream: {polku}"))?;
        let w = v["width"].as_u64().unwrap_or(0) as usize;
        let h = v["height"].as_u64().unwrap_or(0) as usize;
        // Kuvanopeus: kaikki käsittely tehdään kiinteällä (CFR) nopeudella, jolla myös tulos kirjoitetaan,
        // joten tuloksen kesto = lähteen kesto ja ääni pysyy tahdissa. Vaihtuvanopeuksinen lähde (keskiarvo
        // poikkeaa nimellisestä, esim. 24,785 vs 25) muunnetaan nimelliseen nopeuteen kuvia toistamalla/pudottamalla.
        let (ka, kb) = murto(v["avg_frame_rate"].as_str().unwrap_or("0/1"));
        let (ra, rb) = murto(v["r_frame_rate"].as_str().unwrap_or("0/1"));
        let keski = if ka > 0 { ka as f64 / kb as f64 } else { 0.0 };
        let nimellinen = if ra > 0 { ra as f64 / rb as f64 } else { 0.0 };
        let vfr = keski > 0.0 && nimellinen > 0.0 && ((keski - nimellinen) / nimellinen).abs() > 0.002;
        let (a, b) = if vfr && nimellinen <= 120.0 {
            (ra, rb)
        } else if ka > 0 {
            (ka, kb)
        } else if ra > 0 {
            (ra, rb)
        } else {
            (25, 1)
        };
        let fps = a as f64 / b as f64;
        let mut kesto: f64 = v["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if kesto == 0.0 {
            // mkv: kesto tagissa DURATION (hh:mm:ss.nnn)
            if let Some(t) = v["tags"].as_object().and_then(|t| t.iter().find(|(k, _)| k.to_uppercase().starts_with("DURATION"))) {
                let osat: Vec<f64> = t.1.as_str().unwrap_or("").split(':').filter_map(|x| x.parse().ok()).collect();
                if osat.len() == 3 {
                    kesto = osat[0] * 3600.0 + osat[1] * 60.0 + osat[2];
                }
            }
        }
        if kesto == 0.0 {
            kesto = j["format"]["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        }
        let nb: usize = v["nb_frames"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        // vaihtuvanopeuksisessa lähteessä todellinen kuvamäärä ei vastaa kiinteän nopeuden aikajanaa
        let n = if nb > 0 && !vfr { nb } else { (kesto * fps).round() as usize };
        let cs = v["color_space"].as_str().unwrap_or("").to_lowercase();
        let matriisi = match cs.as_str() {
            "bt709" => "bt709",
            "smpte170m" => "smpte170m",
            "bt470bg" => "bt470bg",
            "bt2020nc" => "bt2020",
            _ if h >= 720 => "bt709",
            _ => "smpte170m",
        }
        .to_string();
        Ok(Tiedot {
            polku: polku.to_string(),
            w,
            h,
            fps_osoittaja: a,
            fps_nimittaja: b,
            fps,
            n: n.max(1),
            kesto: if kesto > 0.0 { kesto } else { n as f64 / fps },
            aanta: virrat.iter().any(|s| s["codec_type"] == "audio"),
            matriisi,
            vfr,
            keski_fps: if keski > 0.0 { keski } else { fps },
            harvennus: 0,
            lahde_fps: (a, b),
        })
    }

    /// Kuten `lue`, mutta säännöllisesti toistuvat kopioruudut tunnistetaan ja pudotetaan (todellinen fps).
    pub fn lue_harvennettu(polku: &str) -> Result<Tiedot> {
        let mut t = Tiedot::lue(polku)?;
        if let Some(n) = t.tunnista_kadenssi()? {
            t.harvenna(n);
        }
        Ok(t)
    }

    /// Pudotetaan joka `n`:s kuva: kuvanopeus ja kuvamäärä todelliseksi, kesto ennallaan.
    pub fn harvenna(&mut self, n: usize) {
        let (a, b) = self.lahde_fps;
        let (a, b) = (a * (n as u64 - 1), b * n as u64);
        let g = syt(a, b);
        self.harvennus = n;
        self.fps_osoittaja = a / g;
        self.fps_nimittaja = b / g;
        self.fps = self.fps_osoittaja as f64 / self.fps_nimittaja as f64;
        self.n = ((self.kesto * self.fps).round() as usize).max(1);
    }

    /// Ruudunpudotus ja kiinteä aikajana ffmpegin suodinketjuna.
    fn aikajanasuodin(&self) -> String {
        let (a, b) = self.lahde_fps;
        if self.harvennus > 0 {
            // decimate pudottaa jokaisesta N kuvan jaksosta sen, joka on lähimpänä edellistä (kopio)
            format!("fps={a}/{b},decimate=cycle={}", self.harvennus)
        } else {
            format!("fps={a}/{b}")
        }
    }

    /// Tunnistaa kopioruutujen kadenssin: palauttaa N, jos joka N:s kuva on edellisen kopio
    /// (esim. 25 fps -> 30 fps muunnettu: N = 6, 24 -> 30: N = 5). Näytteet otetaan kolmesta kohdasta.
    /// Tulos muistetaan tiedoston koon ja muokkausajan mukaan.
    pub fn tunnista_kadenssi(&self) -> Result<Option<usize>> {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static MUISTI: OnceLock<Mutex<HashMap<(String, u64, std::time::SystemTime), Option<usize>>>> = OnceLock::new();
        let md = std::fs::metadata(&self.polku)?;
        let avain = (self.polku.clone(), md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH));
        if let Some(v) = MUISTI.get_or_init(Default::default).lock().unwrap().get(&avain) {
            return Ok(*v);
        }
        let mut perus = self.clone();
        perus.harvennus = 0;
        (perus.fps_osoittaja, perus.fps_nimittaja) = perus.lahde_fps;
        perus.fps = perus.fps_osoittaja as f64 / perus.fps_nimittaja as f64;
        let (w, h) = (96, 54);
        let pituus = (20.0 * perus.fps) as usize;
        let alut: Vec<usize> = if perus.kesto > 120.0 {
            [0.15, 0.45, 0.75].iter().map(|x| perus.kehys(perus.kesto * x)).collect()
        } else {
            vec![0]
        };
        let (mut kopiot, mut liike, mut valit) = (0usize, 0usize, Vec::new());
        for alku in alut {
            let kuvat: Vec<Vec<u8>> = Lukija::avaa(&perus, alku, pituus, Some((w, h)))?.map(|k| k.harmaa()).collect();
            let d: Vec<f32> = kuvat.windows(2)
                .map(|p| p[0].iter().zip(&p[1]).map(|(a, b)| (*a as f32 - *b as f32).abs()).sum::<f32>() / (w * h) as f32)
                .collect();
            let mut edellinen: Option<usize> = None;
            for i in 0..d.len() {
                let mut ymp: Vec<f32> = d[i.saturating_sub(6)..(i + 7).min(d.len())].to_vec();
                ymp.sort_by(f32::total_cmp);
                let mediaani = ymp[ymp.len() / 2];
                // vain liikkuvassa kuvassa kopio erottuu varmasti (paikallaan olevassa ero on aina pieni)
                if mediaani <= 1.5 {
                    continue;
                }
                liike += 1;
                if d[i] < 0.35 && d[i] < 0.2 * mediaani {
                    kopiot += 1;
                    if let Some(e) = edellinen {
                        valit.push(i - e);
                    }
                    edellinen = Some(i);
                }
            }
        }
        let mut tulos = None;
        if kopiot >= 20 && valit.len() >= 10 {
            let osuus = kopiot as f64 / liike.max(1) as f64;
            for n in (3..=12).rev() {
                let jaolliset = valit.iter().filter(|&&v| v % n == 0).count() as f64 / valit.len() as f64;
                let uusi_fps = perus.fps * (n - 1) as f64 / n as f64;
                if jaolliset >= 0.8 && osuus > 0.5 / n as f64 && osuus < 1.5 / n as f64 && uusi_fps >= 15.0 {
                    tulos = Some(n);
                    break;
                }
            }
        }
        MUISTI.get_or_init(Default::default).lock().unwrap().insert(avain, tulos);
        Ok(tulos)
    }

    /// Kuvanopeus ffmpegin muodossa (esim. "25/1" tai "30000/1001").
    pub fn fps_teksti(&self) -> String {
        format!("{}/{}", self.fps_osoittaja, self.fps_nimittaja)
    }

    pub fn aika(&self, kehys: usize) -> f64 {
        kehys as f64 / self.fps
    }
    pub fn kehys(&self, s: f64) -> usize {
        (s * self.fps).round().max(0.0) as usize
    }
    /// Esikatselukoko (leveys enintään `leveys`, parilliset mitat).
    pub fn koko_enintaan(&self, leveys: usize) -> (usize, usize) {
        let s = (leveys as f64 / self.w as f64).min(1.0);
        (((self.w as f64 * s) as usize / 2 * 2).max(2), ((self.h as f64 * s) as usize / 2 * 2).max(2))
    }
}

pub fn aikateksti(s: f64) -> String {
    let s = s.max(0.0);
    format!("{}:{:02}:{:06.3}", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64, s % 60.0)
}

pub fn aika_sekunneiksi(t: &str) -> Option<f64> {
    let mut s = 0.0;
    for osa in t.trim().split(':') {
        s = s * 60.0 + osa.trim().replace(',', ".").parse::<f64>().ok()?;
    }
    Some(s)
}

pub struct Lukija {
    lapsi: Child,
    ulos: ChildStdout,
    pub w: usize,
    pub h: usize,
}

impl Lukija {
    /// Lukee kehykset järjestyksessä alkaen kehyksestä `alku`; `maara` = 0 -> loppuun.
    pub fn avaa(t: &Tiedot, alku: usize, maara: usize, koko: Option<(usize, usize)>) -> Result<Lukija> {
        let (w, h) = koko.unwrap_or((t.w, t.h));
        let mut c = komento("ffmpeg")?;
        c.args(["-v", "error", "-nostdin"]);
        if alku > 0 {
            // puolen kehyksen marginaali: ensimmäinen kehys on tasan 'alku'
            c.args(["-ss", &format!("{:.6}", (alku as f64 - 0.5) / t.fps)]);
        }
        c.arg("-i").arg(&t.polku);
        c.args(["-map", "0:v:0", "-an", "-sn", "-vf"]);
        // fps-suodin: kiinteä aikajana (vaihtuvanopeuksinen lähde muunnetaan; kiinteällä lähteellä ei vaikutusta),
        // tarvittaessa kopioruutujen pudotus
        c.arg(format!("{},scale={w}:{h}:flags=area+accurate_rnd:in_color_matrix={}", t.aikajanasuodin(), t.matriisi));
        if maara > 0 {
            c.args(["-frames:v", &maara.to_string()]);
        }
        c.args(["-pix_fmt", "bgr24", "-f", "rawvideo", "-"]);
        let mut lapsi = c.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn().context("ffmpeg did not start")?;
        let ulos = lapsi.stdout.take().unwrap();
        Ok(Lukija { lapsi, ulos, w, h })
    }

    pub fn seuraava(&mut self) -> Option<Bgr> {
        let mut k = Bgr::new(self.w, self.h);
        match self.ulos.read_exact(&mut k.data) {
            Ok(()) => Some(k),
            Err(_) => None,
        }
    }
}

impl Iterator for Lukija {
    type Item = Bgr;
    fn next(&mut self) -> Option<Bgr> {
        self.seuraava()
    }
}

impl Drop for Lukija {
    fn drop(&mut self) {
        let _ = self.lapsi.kill();
        let _ = self.lapsi.wait();
    }
}

pub fn hae_kehys(t: &Tiedot, kehys: usize, koko: Option<(usize, usize)>) -> Result<Bgr> {
    Lukija::avaa(t, kehys, 1, koko)?.seuraava().ok_or_else(|| anyhow!("Could not read frame {kehys}."))
}

pub struct Kirjoittaja {
    lapsi: Child,
    sisaan: Option<ChildStdin>,
    loki: PathBuf,
}

pub struct Kirjoitusasetukset<'a> {
    pub aanilahde: Option<&'a str>,
    pub aanilahde_ulkopuolinen: bool, // true = ulkopuolinen tiedosto, ei video lähde
    pub alku_s: f64,
    pub kesto_s: f64,
    pub koodekki: &'a str,
    pub crf: u32,
}

impl Kirjoittaja {
    pub fn avaa(ulos: &str, t: &Tiedot, a: &Kirjoitusasetukset) -> Result<Kirjoittaja> {
        let ulos_p = Path::new(ulos);
        if let Some(k) = ulos_p.parent() {
            std::fs::create_dir_all(k).ok();
        }
        let mut c = komento("ffmpeg")?;
        c.args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgr24", "-s", &format!("{}x{}", t.w, t.h)]);
        c.args(["-r", &t.fps_teksti(), "-i", "-"]);
        let aani = match a.aanilahde {
            Some(p) => Tiedot::lue(p).map(|x| x.aanta).unwrap_or(false),
            None => false,
        };
        if aani {
            if !a.aanilahde_ulkopuolinen {
                // Video lähteen ääni: seekea ja leikkaa videon segmentin mukaan
                if a.alku_s > 0.0 {
                    c.args(["-ss", &format!("{:.6}", a.alku_s)]);
                }
                if a.kesto_s > 0.0 {
                    c.args(["-t", &format!("{:.6}", a.kesto_s)]);
                }
            }
            // Ulkopuolinen ääni: ei seekkausta/leikkausta, alkaa alusta ja toistaa kokonaisen pätkän
            c.arg("-i").arg(a.aanilahde.unwrap());
            c.args(["-map", "0:v", "-map", "1:a?"]);
            // kopiointi leikkaa vain pakettien rajoilta, joten pätkät koodataan uudelleen
            let mkv = ulos_p.extension().map(|e| e.eq_ignore_ascii_case("mkv")).unwrap_or(false);
            if mkv && a.alku_s == 0.0 && a.kesto_s == 0.0 && !a.aanilahde_ulkopuolinen {
                c.args(["-c:a", "copy"]);
            } else {
                c.args(["-c:a", "aac", "-b:a", "192k"]);
            }
        }
        let m = t.matriisi.as_str();
        c.args(["-vf", &format!("scale=out_color_matrix={m}:out_range=tv,format=yuv420p")]);
        let crf = a.crf.to_string();
        match a.koodekki {
            "x265" => c.args(["-c:v", "libx265", "-preset", "slow", "-crf", &crf, "-x265-params", "log-level=error"]),
            "nvenc" => c.args(["-c:v", "h264_nvenc", "-preset", "p6", "-cq", &crf, "-b:v", "0"]),
            _ => c.args(["-c:v", "libx264", "-preset", "slow", "-crf", &crf]),
        };
        let (prim, trc) = if m == "bt709" { ("bt709", "bt709") } else { ("smpte170m", "smpte170m") };
        // -shortest vain videon oman äänen kanssa; ulkopuolisella ääniä ei rajoiteta
        if a.aanilahde_ulkopuolinen {
            c.args(["-colorspace", m, "-color_primaries", prim, "-color_trc", trc]);
        } else {
            c.args(["-colorspace", m, "-color_primaries", prim, "-color_trc", trc, "-shortest"]);
        }
        c.arg(ulos);
        let loki = PathBuf::from(format!("{ulos}.ffmpeg.log"));
        let loki_f = std::fs::File::create(&loki)?;
        let mut lapsi = c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(loki_f).spawn().context("ffmpeg did not start")?;
        let sisaan = lapsi.stdin.take();
        Ok(Kirjoittaja { lapsi, sisaan, loki })
    }

    pub fn kirjoita(&mut self, k: &Bgr) -> Result<()> {
        let s = self.sisaan.as_mut().ok_or_else(|| anyhow!("writer closed"))?;
        if s.write_all(&k.data).is_err() {
            bail!("{}", self.virhe());
        }
        Ok(())
    }

    fn virhe(&self) -> String {
        let t = std::fs::read_to_string(&self.loki).unwrap_or_default();
        let t = t.trim();
        format!("writing with ffmpeg failed: {}", &t[t.len().saturating_sub(800)..])
    }

    pub fn sulje(mut self) -> Result<()> {
        drop(self.sisaan.take());
        let s = self.lapsi.wait()?;
        if !s.success() {
            bail!("{}", self.virhe());
        }
        if std::fs::metadata(&self.loki).map(|m| m.len() == 0).unwrap_or(false) {
            let _ = std::fs::remove_file(&self.loki);
        }
        Ok(())
    }

    pub fn keskeyta(mut self) {
        drop(self.sisaan.take());
        let _ = self.lapsi.kill();
        let _ = self.lapsi.wait();
    }
}
