//! Lisäosat: perusasennus sisältää vain Varita.exe:n ja DirectML.dll:n. Kaikki muu (ffmpeg, väritysmallit,
//! näkömalli Ollamaan) asennetaan sovelluksen omaan kansioon Lisäosat-välilehdeltä ensimmäisen käynnistyksen jälkeen.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use crate::mallit;
use crate::video::sovelluskansio;

/// BtbN:n GPL-käännös (sisältää libx264:n). Lisenssi: GPL-3.0 – kopio tallennetaan ffmpeg\LICENSE.txt.
pub const FFMPEG_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
pub const FFMPEG_KOKO_MT: u32 = 191;

pub enum Lahde {
    /// Ladataan verkosta sovelluksen kansioon.
    Verkko,
    /// Ei julkista latauslähdettä – tuodaan tiedostosta (esim. oma ONNX-vienti).
    Tiedosto,
}

pub struct Osa {
    pub tunnus: String,
    pub nimi: String,
    pub kuvaus: String,
    pub koko_mt: u32,
    pub pakollinen: bool,
    pub lisenssi: &'static str,
    pub lahde: Lahde,
    /// Asennettu: sijainti tai lisätieto.
    pub asennettu: Option<String>,
}

/// Sovelluksen oma ffmpeg (ffmpeg\bin).
pub fn ffmpeg_oma() -> Option<PathBuf> {
    let b = sovelluskansio().join("ffmpeg").join("bin");
    (b.join("ffmpeg.exe").is_file() && b.join("ffprobe.exe").is_file()).then_some(b)
}

fn mallin_lisenssi(t: &mallit::MalliTieto) -> &'static str {
    match t.tyyppi {
        mallit::Tyyppi::DDColor => "Apache-2.0 (DDColor)",
    }
}

/// Lisäosat ilman Ollama-tilaa (se haetaan erikseen, koska palvelimen käynnistys kestää).
pub fn osat() -> Vec<Osa> {
    let mut v = vec![Osa {
        tunnus: "ffmpeg".into(),
        nimi: "FFmpeg".into(),
        kuvaus: "Video reading and writing, audio (required)".into(),
        koko_mt: FFMPEG_KOKO_MT,
        pakollinen: true,
        lisenssi: "GPL-3.0 (FFmpeg, BtbN build)",
        lahde: Lahde::Verkko,
        asennettu: ffmpeg_oma().map(|p| p.display().to_string()).or_else(|| {
            crate::video::ohjelma("ffmpeg").ok().map(|p| format!("{} (system PATH)", p.display()))
        }),
    }];
    for m in mallit::MALLIT {
        v.push(Osa {
            tunnus: format!("malli:{}", m.nimi),
            nimi: m.nimi.into(),
            kuvaus: m.kuvaus.into(),
            koko_mt: m.koko_mt,
            pakollinen: m.nimi == "ddcolor_suomi",
            lisenssi: mallin_lisenssi(m),
            lahde: if m.url.is_some() { Lahde::Verkko } else { Lahde::Tiedosto },
            asennettu: mallit::polku(m.nimi).map(|p| p.display().to_string()),
        });
    }
    v.push(Osa {
        tunnus: format!("piirteet:{}", crate::piirteet::NIMI),
        nimi: crate::piirteet::NIMI.into(),
        kuvaus: crate::piirteet::KUVAUS.into(),
        koko_mt: crate::piirteet::KOKO_MT,
        pakollinen: false,
        lisenssi: crate::piirteet::LISENSSI,
        lahde: Lahde::Verkko,
        asennettu: crate::piirteet::polku().map(|p| p.display().to_string()),
    });
    v
}

/// Puuttuuko jotain, jota ilman väritys ei toimi (ffmpeg tai yksikään DDColor-malli)?
pub fn puuttuu_pakollisia() -> bool {
    crate::video::ohjelma("ffmpeg").is_err()
        || crate::video::ohjelma("ffprobe").is_err()
        || !mallit::ddcolor_mallit().iter().any(|m| mallit::on_ladattu(m))
}

/// Lataa tiedoston (ensin .part, valmiina nimetään). Palauttaa CRC32-tarkisteen.
pub fn lataa_tiedosto(url: &str, kohde: &Path, edistys: &mut dyn FnMut(u64, u64), seis: &AtomicBool) -> Result<u32> {
    std::fs::create_dir_all(kohde.parent().ok_or_else(|| anyhow!("invalid target"))?)?;
    let osa = PathBuf::from(format!("{}.part", kohde.display()));
    let vastaus = ureq::get(url).call().with_context(|| format!("download failed: {url}"))?;
    let koko: u64 = vastaus.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut lukija = vastaus.into_body().into_reader();
    let mut f = std::fs::File::create(&osa)?;
    let mut puskuri = vec![0u8; 1 << 20];
    let mut saatu = 0u64;
    let mut crc = crc32fast::Hasher::new();
    let tulos = (|| -> Result<()> {
        loop {
            if seis.load(Ordering::Relaxed) {
                bail!("Download cancelled.");
            }
            let n = lukija.read(&mut puskuri)?;
            if n == 0 {
                break;
            }
            f.write_all(&puskuri[..n])?;
            crc.update(&puskuri[..n]);
            saatu += n as u64;
            edistys(saatu, koko);
        }
        if koko > 0 && saatu != koko {
            bail!("download interrupted ({saatu}/{koko} bytes)");
        }
        Ok(())
    })();
    drop(f);
    if let Err(e) = tulos {
        let _ = std::fs::remove_file(&osa);
        return Err(e);
    }
    std::fs::rename(&osa, kohde)?;
    Ok(crc.finalize())
}

/// Lataa ffmpegin, purkaa ffmpeg.exe:n, ffprobe.exe:n ja lisenssin sovelluksen ffmpeg-kansioon ja kokeilee ne.
pub fn asenna_ffmpeg(edistys: &mut dyn FnMut(u64, u64), seis: &AtomicBool) -> Result<PathBuf> {
    let kansio = sovelluskansio().join("ffmpeg");
    let bin = kansio.join("bin");
    let zip_polku = kansio.join("ffmpeg-lataus.zip");
    lataa_tiedosto(FFMPEG_URL, &zip_polku, edistys, seis)?;
    let tulos = pura_ffmpeg(&zip_polku, &kansio);
    let _ = std::fs::remove_file(&zip_polku);
    tulos?;
    for exe in ["ffmpeg", "ffprobe"] {
        let ok = std::process::Command::new(bin.join(format!("{exe}.exe")))
            .arg("-version")
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            bail!("{exe}.exe was installed but does not start.");
        }
    }
    Ok(bin)
}

fn pura_ffmpeg(zip_polku: &Path, kansio: &Path) -> Result<()> {
    let mut arkisto = zip::ZipArchive::new(std::fs::File::open(zip_polku)?).context("the ffmpeg package is corrupted")?;
    let bin = kansio.join("bin");
    std::fs::create_dir_all(&bin)?;
    let mut loydetyt = 0;
    for i in 0..arkisto.len() {
        let mut e = arkisto.by_index(i)?;
        let nimi = e.name().replace('\\', "/");
        let kohde = if nimi.ends_with("/bin/ffmpeg.exe") {
            bin.join("ffmpeg.exe")
        } else if nimi.ends_with("/bin/ffprobe.exe") {
            bin.join("ffprobe.exe")
        } else if nimi.ends_with("/LICENSE.txt") && nimi.matches('/').count() == 1 {
            kansio.join("LICENSE.txt")
        } else {
            continue;
        };
        let valiaikainen = kohde.with_extension("uusi");
        let mut f = std::fs::File::create(&valiaikainen)?;
        // zip-kirjasto tarkistaa jokaisen tiedoston CRC32:n luettaessa
        std::io::copy(&mut e, &mut f).with_context(|| format!("extracting {nimi} failed"))?;
        drop(f);
        std::fs::rename(&valiaikainen, &kohde)?;
        if !nimi.ends_with("LICENSE.txt") {
            loydetyt += 1;
        }
    }
    if loydetyt < 2 {
        bail!("ffmpeg.exe and ffprobe.exe were not found in the ffmpeg package");
    }
    Ok(())
}

pub fn poista_ffmpeg() -> Result<()> {
    let k = sovelluskansio().join("ffmpeg");
    if k.is_dir() {
        std::fs::remove_dir_all(&k).with_context(|| format!("removing {} failed (is a video still playing?)", k.display()))?;
    }
    Ok(())
}

/// Ollaman tila Lisäosat-välilehdelle.
pub struct OllamaTila {
    pub asennettu: bool,
    pub kaynnissa: bool,
    pub nakomallit: Vec<String>,
    pub virhe: Option<String>,
}

pub fn ollama_tila() -> OllamaTila {
    let asennettu = crate::nako::ollama_exe().is_some() || crate::nako::kaynnissa();
    if !asennettu {
        return OllamaTila { asennettu, kaynnissa: false, nakomallit: vec![], virhe: None };
    }
    match crate::nako::nakomallit() {
        Ok(m) => OllamaTila { asennettu, kaynnissa: true, nakomallit: m, virhe: None },
        Err(e) => OllamaTila { asennettu, kaynnissa: crate::nako::kaynnissa(), nakomallit: vec![], virhe: Some(format!("{e:#}")) },
    }
}
