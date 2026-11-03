"""Palauttaa elokuva-aineiston lähdeluettelon kuvatiedostojen nimistä: <etuliite>_<IA-tunnus[:60]>_<nro>.jpg.
Jokaisen elokuvan tiedot (nimi, vuosi, tekijä, lisenssi) haetaan Internet Archivesta uudelleen.

Käyttö: python palauta_lahteet.py KANSIO ETULIITE [--huomautus TEKSTI]
"""
import argparse, collections, json, time, urllib.parse, urllib.request
from pathlib import Path

OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (avoin varityssovellus)"}


def hae(url):
    for i in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=60) as v:
                return json.loads(v.read())
        except Exception:
            time.sleep(3 * (i + 1))
    return {}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kansio")
    p.add_argument("etuliite")
    p.add_argument("--huomautus", default="")
    a = p.parse_args()
    k = Path(a.kansio)
    laskuri = collections.Counter(f.stem[len(a.etuliite) + 1:].rsplit("_", 1)[0] for f in (k / "kuvat").glob("*.jpg"))
    ulos = k / f"lahteet_{a.etuliite}.jsonl"
    rivit, puutteet = [], 0
    for n, (tunnus, kuvia) in enumerate(sorted(laskuri.items()), 1):
        md = hae(f"https://archive.org/metadata/{urllib.parse.quote(tunnus)}")
        if not md.get("metadata") and len(tunnus) >= 60:
            # tiedostonimessä tunnus on katkaistu 60 merkkiin: haetaan alkuosalla
            q = urllib.parse.urlencode([("q", f"identifier:{tunnus}*"), ("fl[]", "identifier"), ("rows", "2"), ("output", "json")])
            docs = hae("https://archive.org/advancedsearch.php?" + q).get("response", {}).get("docs", [])
            if len(docs) == 1:
                md = hae(f"https://archive.org/metadata/{urllib.parse.quote(docs[0]['identifier'])}")
        meta = md.get("metadata") or {}
        if not meta:
            puutteet += 1
        lisenssi_url = meta.get("licenseurl", "")
        rivit.append({"id": meta.get("identifier", tunnus), "otsikko": meta.get("title"), "vuosi": meta.get("year") or meta.get("date"),
                      "tekija": meta.get("creator"), "lisenssi_url": lisenssi_url,
                      "lisenssi": "Public Domain" + (f" ({a.huomautus})" if a.huomautus else ""),
                      "lahde": f"https://archive.org/details/{meta.get('identifier', tunnus)}", "kuvia": kuvia, "tila": "vari"})
        if n % 50 == 0:
            print(f"{n}/{len(laskuri)}", flush=True)
        time.sleep(0.3)
    ulos.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rivit) + "\n", encoding="utf-8")
    ilman_pd = [r["id"] for r in rivit if "publicdomain" not in str(r["lisenssi_url"])]
    print(f"{ulos}: {len(rivit)} elokuvaa, tietoja ei löytynyt {puutteet}, ilman PD-lisenssimerkintää {len(ilman_pd)}")
    (k / "ilman_pd_merkintaa.txt").write_text("\n".join(ilman_pd), encoding="utf-8")


if __name__ == "__main__":
    main()
