"""Internet Archive / Prelinger Archives: värielokuvat 1950–1980, joiden lisenssiksi on merkitty
public domain. Lataa 640x480-version ja poimii kuvat noin 2 s välein (vain värilliset, ei lähes
samoja peräkkäisiä). Lähteet kirjataan tiedostoon lahteet_prelinger.jsonl.

Käyttö: python prelinger_lataa.py KOHDEKANSIO FFMPEG.EXE
"""
import io, json, subprocess, sys, time, urllib.parse, urllib.request
from pathlib import Path

import numpy as np
from PIL import Image

OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (avoin varityssovellus)"}
HAKU = ("collection:prelinger AND color:(color) AND year:[1950 TO 1980] "
        "AND licenseurl:*publicdomain*")


def hae(url, yrityksia=4):
    for i in range(yrityksia):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=120) as v:
                return v.read()
        except Exception:
            if i == yrityksia - 1:
                raise
            time.sleep(3 * (i + 1))


def lataa(url, kohde):
    osa = kohde.with_suffix(kohde.suffix + ".part")
    with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=120) as v, open(osa, "wb") as f:
        while True:
            b = v.read(1 << 20)
            if not b:
                break
            f.write(b)
    osa.replace(kohde)


def varillinen(k):
    a = k.astype(np.float32)
    rg, yb = a[..., 0] - a[..., 1], 0.5 * (a[..., 0] + a[..., 1]) - a[..., 2]
    return np.sqrt(rg.std() ** 2 + yb.std() ** 2) + 0.3 * np.sqrt(rg.mean() ** 2 + yb.mean() ** 2)


def main():
    kohde, ffmpeg = Path(sys.argv[1]), sys.argv[2]
    (kohde / "video").mkdir(parents=True, exist_ok=True)
    (kohde / "kuvat").mkdir(parents=True, exist_ok=True)
    luettelo = kohde / "lahteet_prelinger.jsonl"
    tehdyt = set()
    if luettelo.exists():
        tehdyt = {json.loads(r)["id"] for r in luettelo.read_text(encoding="utf-8").splitlines()}
    q = urllib.parse.urlencode([("q", HAKU), ("fl[]", "identifier"), ("rows", "500"), ("output", "json")])
    tunnukset = [d["identifier"] for d in json.loads(hae("https://archive.org/advancedsearch.php?" + q))["response"]["docs"]]
    print(f"Prelinger: {len(tunnukset)} public domain -värielokuvaa", flush=True)
    for n, t in enumerate(tunnukset, 1):
        if t in tehdyt:
            continue
        try:
            md = json.loads(hae(f"https://archive.org/metadata/{t}"))
            meta = md["metadata"]
            if "publicdomain" not in str(meta.get("licenseurl", "")):
                continue
            tiedostot = md["files"]
            valinta = next((f for f in tiedostot if f.get("format") == "h.264" and f["name"].endswith(".mp4")), None) \
                or next((f for f in tiedostot if f.get("format") == "MPEG2"), None)
            if not valinta:
                print(f"{t}: ei sopivaa videota", flush=True)
                continue
            video = kohde / "video" / (t + Path(valinta["name"]).suffix)
            if not video.exists():
                lataa(f"https://archive.org/download/{t}/{urllib.parse.quote(valinta['name'])}", video)
            # kuvat 2 s välein, 640x480 -> kehykset muistiin
            w, h = 640, 480
            raaka = subprocess.run([ffmpeg, "-v", "error", "-i", str(video), "-vf", f"fps=1/2,scale={w}:{h}",
                                    "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
            kuvat = np.frombuffer(raaka, np.uint8).reshape(-1, h, w, 3)
            ed, talletettu = None, 0
            for i, k in enumerate(kuvat):
                if varillinen(k[::4, ::4]) < 15.0:
                    continue
                pieni = k[::8, ::8].astype(np.float32).mean(axis=2)
                if pieni.std() < 12.0:  # tasainen ruutu, häivytys tai teksti mustalla
                    continue
                if ed is not None and np.abs(pieni - ed).mean() < 6.0:
                    continue
                ed = pieni
                Image.fromarray(k).save(kohde / "kuvat" / f"prelinger_{t[:60]}_{i:05d}.jpg", quality=95)
                talletettu += 1
            tieto = {"id": t, "otsikko": meta.get("title"), "vuosi": meta.get("year"), "tekija": meta.get("creator"),
                     "lisenssi": "Public Domain", "lisenssi_url": meta.get("licenseurl"),
                     "lahde": f"https://archive.org/details/{t}", "kuvia": talletettu}
            luettelo.open("a", encoding="utf-8").write(json.dumps(tieto, ensure_ascii=False) + "\n")
            print(f"{n}/{len(tunnukset)} {t}: {talletettu} kuvaa", flush=True)
        except Exception as e:
            print(f"{t}: {e}", flush=True)


if __name__ == "__main__":
    main()
