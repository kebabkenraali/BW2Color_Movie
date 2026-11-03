"""Internet Archive -elokuvista näytekuvat koulutukseen ilman koko videon latausta: PyAV avaa videon
verkosta kerran ja hakee kuvat suoraan oikeista kohdista (HTTP range). Ensin 6 kuvan väritarkistus –
mustavalkoiset elokuvat ohitetaan – sitten N tasavälistä kuvaa. Mustat reunapalkit rajataan, tekstiruudut
ja pimeät ruudut hylätään. Lähteet kirjataan tiedostoon lahteet_<nimi>.jsonl.

Ajetaan omassa virtuaaliympäristössä (.venv_lataus: av, numpy, pillow).
Käyttö: python elokuvat_lataa.py KOHDE NIMI "IA-HAKU" [--kuvia 60] [--rinnakkain 6] [--huomautus TEKSTI]
"""
import argparse, json, multiprocessing as mp, sys, time, urllib.parse, urllib.request
from pathlib import Path

import av
import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))
from kuvasuodatin import hylkaa  # noqa: E402

OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (avoin varityssovellus)"}
MUODOT = ("h.264", "h.264 IA", "MPEG4", "512Kb MPEG4")


def hae(url, yrityksia=4):
    for i in range(yrityksia):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=120) as v:
                return v.read()
        except Exception:
            if i == yrityksia - 1:
                raise
            time.sleep(5 * (i + 1))


def varikkyys(k):
    a = k.astype(np.float32)
    rg, yb = a[..., 0] - a[..., 1], 0.5 * (a[..., 0] + a[..., 1]) - a[..., 2]
    return float(np.sqrt(rg.std() ** 2 + yb.std() ** 2) + 0.3 * np.sqrt(rg.mean() ** 2 + yb.mean() ** 2))


def rajaa(k):
    """Mustat reunapalkit pois (laajakuva, ikkunointi)."""
    g = k.mean(2)
    rivit = np.where(g.mean(1) > 14)[0]
    sarakkeet = np.where(g.mean(0) > 14)[0]
    if len(rivit) < k.shape[0] * 0.4 or len(sarakkeet) < k.shape[1] * 0.4:
        return None
    return k[rivit[0]:rivit[-1] + 1, sarakkeet[0]:sarakkeet[-1] + 1]


def kelpaa(k):
    pieni = k[::4, ::4]
    if not (varikkyys(pieni) >= 15.0 and pieni.mean(2).std() >= 12.0 and 20 < pieni.mean() < 235):
        return False
    return hylkaa(np.ascontiguousarray(k)) is None  # haalistunut filmi, tekstiruutu, tyhjä ruutu


def kuva_kohdasta(c, s, t):
    c.seek(int(t / s.time_base), stream=s)
    for f in c.decode(s):
        k = f.to_ndarray(format="rgb24")
        if k.shape[1] > 640:
            k = np.asarray(Image.fromarray(k).resize((640, int(k.shape[0] * 640 / k.shape[1])), Image.BICUBIC))
        return k
    return None


def kasittele(tehtava):
    tunnus, kohde, nimi, kuvia, huomautus, vaadi_pd = tehtava
    tulos = {"id": tunnus, "lahde": f"https://archive.org/details/{tunnus}"}
    try:
        md = json.loads(hae(f"https://archive.org/metadata/{tunnus}"))
        meta = md.get("metadata", {})
        tulos.update({"otsikko": meta.get("title"), "vuosi": meta.get("year") or meta.get("date"),
                      "tekija": meta.get("creator"), "lisenssi_url": meta.get("licenseurl"),
                      "lisenssi": "Public Domain" + (f" ({huomautus})" if huomautus else "")})
        if vaadi_pd and "publicdomain" not in str(meta.get("licenseurl", "")):
            return {**tulos, "tila": "ei PD-lisenssiä", "kuvia": 0}
        tiedosto = next((f for m in MUODOT for f in md.get("files", []) if f.get("format") == m and f["name"].endswith(".mp4")), None)
        if not tiedosto:
            return {**tulos, "tila": "ei videota", "kuvia": 0}
        url = f"https://archive.org/download/{tunnus}/{urllib.parse.quote(tiedosto['name'])}"
        c = av.open(url, options={"reconnect": "1", "timeout": "30000000"}, timeout=60)
        s = c.streams.video[0]
        s.thread_type = "AUTO"
        kesto = float(c.duration / av.time_base) if c.duration else float(tiedosto.get("length", 0) or 0)
        if kesto < 60:
            c.close()
            return {**tulos, "tila": "liian lyhyt", "kuvia": 0}
        # väritarkistus
        tarkistus = [kuva_kohdasta(c, s, kesto * x) for x in (0.15, 0.3, 0.45, 0.6, 0.75, 0.9)]
        varillisia = sum(1 for k in tarkistus if k is not None and varikkyys(k[::4, ::4]) >= 15.0)
        if varillisia < 3:
            c.close()
            return {**tulos, "tila": "mustavalko", "kuvia": 0}
        talletettu = 0
        for i, x in enumerate(np.linspace(0.03, 0.97, kuvia)):
            try:
                k = kuva_kohdasta(c, s, kesto * x)
            except Exception:
                continue
            if k is None:
                continue
            k = rajaa(k)
            if k is None or not kelpaa(k):
                continue
            Image.fromarray(k).save(Path(kohde) / "kuvat" / f"{nimi}_{tunnus[:60]}_{i:03d}.jpg", quality=95)
            talletettu += 1
        c.close()
        return {**tulos, "tila": "vari", "kuvia": talletettu}
    except Exception as e:
        return {**tulos, "tila": f"virhe: {str(e)[:120]}", "kuvia": 0}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kohde")
    p.add_argument("nimi")
    p.add_argument("haku")
    p.add_argument("--kuvia", type=int, default=60)
    p.add_argument("--rinnakkain", type=int, default=6)
    p.add_argument("--huomautus", default="")
    p.add_argument("--vaadi-pd", action="store_true", help="vain elokuvat, joiden licenseurl on public domain")
    p.add_argument("--ohita", nargs="*", default=[], help="lähdeluettelot, joiden elokuvat on jo ladattu muualle")
    a = p.parse_args()
    kohde = Path(a.kohde)
    (kohde / "kuvat").mkdir(parents=True, exist_ok=True)
    luettelo = kohde / f"lahteet_{a.nimi}.jsonl"
    tehdyt = set()
    if luettelo.exists():
        tehdyt = {json.loads(r)["id"] for r in luettelo.read_text(encoding="utf-8").splitlines() if r.strip()}
    for o in a.ohita:
        tehdyt |= {json.loads(r)["id"] for r in Path(o).read_text(encoding="utf-8").splitlines() if r.strip()}
    q = urllib.parse.urlencode([("q", a.haku), ("fl[]", "identifier"), ("rows", "10000"), ("output", "json")])
    tunnukset = [d["identifier"] for d in json.loads(hae("https://archive.org/advancedsearch.php?" + q))["response"]["docs"]]
    jaljella = [t for t in tunnukset if t not in tehdyt]
    print(f"{a.nimi}: {len(tunnukset)} elokuvaa, käsittelemättä {len(jaljella)}", flush=True)
    tilasto = {"vari": 0, "kuvia": 0, "muut": 0}
    with mp.Pool(a.rinnakkain) as pool:
        for n, r in enumerate(pool.imap_unordered(kasittele, [(t, str(kohde), a.nimi, a.kuvia, a.huomautus, a.vaadi_pd) for t in jaljella]), 1):
            luettelo.open("a", encoding="utf-8").write(json.dumps(r, ensure_ascii=False) + "\n")
            if r["tila"] == "vari":
                tilasto["vari"] += 1
                tilasto["kuvia"] += r["kuvia"]
            else:
                tilasto["muut"] += 1
            if n % 10 == 0:
                print(f"{n}/{len(jaljella)}: värielokuvia {tilasto['vari']}, kuvia {tilasto['kuvia']}, muita {tilasto['muut']}", flush=True)
    print(f"Valmis: {tilasto}", flush=True)


if __name__ == "__main__":
    main()
