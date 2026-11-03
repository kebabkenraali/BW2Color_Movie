"""Finna.fi: vapaasti käytettävät (usage_A: CC0, PDM, CC BY 4.0) valokuvat vuosilta 1950–1990.
Vain värikuvat säilytetään (mustavalkoiset ja seepiasävytetyt hylätään). Jokaisesta kuvasta
kirjataan tekijä, arkisto ja lisenssi tiedostoon lahteet_finna.jsonl (tekijämaininta julkaisussa).

Käyttö: python finna_lataa.py KOHDEKANSIO [--alku 1950] [--loppu 1990]
"""
import argparse, io, json, sys, threading, time, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

API = "https://api.finna.fi/v1/search"
KENTAT = ["id", "title", "year", "images", "imageRights", "nonPresenterAuthors", "buildings"]
OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (avoin varityssovellus)"}


def hae(url, yrityksia=4):
    for i in range(yrityksia):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=60) as v:
                return v.read()
        except Exception:
            if i == yrityksia - 1:
                raise
            time.sleep(2 * (i + 1))


def sivu(alku, loppu, n):
    q = [("lookfor", "*"), ("limit", "100"), ("page", str(n)),
         ("filter[]", 'usage_rights_str_mv:"usage_A"'), ("filter[]", 'format:"0/Image/"'),
         ("filter[]", f'search_daterange_mv:"[{alku} TO {loppu}]"')] + [("field[]", k) for k in KENTAT]
    return json.loads(hae(API + "?" + urllib.parse.urlencode(q)))


def varikuva(im):
    """True, jos kuva on aidosti värillinen: riittävä värikkyys (Hasler–Süsstrunk) ja sävyt kahteen
    suuntaan (sävytetty mustavalkokuva, esim. seepia, vaihtelee vain yhtä akselia). Mitataan kuvan
    keskiosasta, ettei paspartuun tai pahvikehyksen väri vaikuta."""
    w, h = im.size
    im = im.convert("RGB").crop((int(w * 0.15), int(h * 0.15), int(w * 0.85), int(h * 0.85))).resize((256, 256))
    a = np.asarray(im, dtype=np.float32)
    ab = np.asarray(im.convert("LAB"), dtype=np.float32)[..., 1:].reshape(-1, 2) - 128.0
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    rg, yb = r - g, 0.5 * (r + g) - b
    m = np.sqrt(rg.std() ** 2 + yb.std() ** 2) + 0.3 * np.sqrt(rg.mean() ** 2 + yb.mean() ** 2)
    e = np.linalg.eigvalsh(np.cov((ab - ab.mean(0)).T))
    pieni, suuri = np.sqrt(max(e[0], 0.0)), np.sqrt(max(e[1], 1e-6))
    return bool(m > 18.0 and pieni > 4.0 and pieni / suuri > 0.22), float(m), float(pieni)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kohde")
    p.add_argument("--alku", type=int, default=1950)
    p.add_argument("--loppu", type=int, default=1990)
    p.add_argument("--saikeita", type=int, default=6)
    p.add_argument("--enintaan", type=int, default=0, help="lopeta, kun näin monta värikuvaa on hyväksytty (0 = ei rajaa)")
    a = p.parse_args()
    kohde = Path(a.kohde)
    (kohde / "kuvat").mkdir(parents=True, exist_ok=True)
    luettelo = kohde / "lahteet_finna.jsonl"
    hylatyt_p = kohde / "hylatyt_finna.txt"
    tehdyt = set()
    for f in (luettelo, hylatyt_p):
        if f.exists():
            for rivi in f.read_text(encoding="utf-8").splitlines():
                tehdyt.add(json.loads(rivi)["id"] if rivi.startswith("{") else rivi.strip())
    lukko = threading.Lock()
    tilasto = {"ok": 0, "hyl": 0, "virhe": 0}

    def kasittele(r):
        rid = r["id"]
        if rid in tehdyt or not r.get("images"):
            return
        try:
            data = hae("https://finna.fi" + r["images"][0])
            im = Image.open(io.BytesIO(data))
            ok, m, pieni = varikuva(im)
        except Exception:
            with lukko:
                tilasto["virhe"] += 1
            return
        with lukko:
            if not ok or not r.get("imageRights", {}).get("copyright"):  # vain nimetty vapaa lisenssi
                tilasto["hyl"] += 1
                hylatyt_p.open("a", encoding="utf-8").write(rid + "\n")
                return
            nimi = "finna_" + "".join(c if c.isalnum() else "_" for c in rid)[:80] + ".jpg"
            im.convert("RGB").save(kohde / "kuvat" / nimi, quality=95)
            oikeus = r.get("imageRights", {})
            tieto = {
                "id": rid, "tiedosto": nimi, "otsikko": r.get("title"), "vuosi": r.get("year"),
                "tekija": [x.get("name") for x in r.get("nonPresenterAuthors", [])],
                "arkisto": (r.get("buildings") or [{}])[0].get("translated"),
                "lisenssi": oikeus.get("copyright"), "lisenssi_url": oikeus.get("link"),
                "lahde": f"https://www.finna.fi/Record/{urllib.parse.quote(rid)}", "varikkyys": round(m, 1),
            }
            luettelo.open("a", encoding="utf-8").write(json.dumps(tieto, ensure_ascii=False) + "\n")
            tilasto["ok"] += 1

    eka = sivu(a.alku, a.loppu, 1)
    yht = eka["resultCount"]
    sivuja = (yht + 99) // 100
    print(f"Finna: {yht} vapaata kuvaa {a.alku}–{a.loppu}, {sivuja} sivua", flush=True)
    with ThreadPoolExecutor(a.saikeita) as ex:
        for n in range(1, sivuja + 1):
            try:
                d = eka if n == 1 else sivu(a.alku, a.loppu, n)
            except Exception as e:
                print(f"sivu {n}: {e}", flush=True)
                continue
            list(ex.map(kasittele, d.get("records", [])))
            if a.enintaan and tilasto["ok"] >= a.enintaan:
                print(f"enimmäismäärä {a.enintaan} saavutettu", flush=True)
                break
            if n % 10 == 0:
                print(f"sivu {n}/{sivuja}: värikuvia {tilasto['ok']}, hylätty {tilasto['hyl']}, virheitä {tilasto['virhe']}", flush=True)
    print(f"Valmis: värikuvia {tilasto['ok']}, hylätty {tilasto['hyl']}, virheitä {tilasto['virhe']}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
