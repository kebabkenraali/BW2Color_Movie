"""Filmityylin luokittelu koulutusaineistolle: jokaiselle elokuvalle tyyli Internet Archiven kuvauksen
perusteella. Tulos: aineisto/tyylit.json {"<etuliite>_<tunnus[:60]>": tyyli, ...} ja lähdekohtaiset oletukset.

Tyylit (sama järjestys kuin mallin tyyliupotuksessa ja Väritän mallilistassa):
  0 automaattinen (ei ehtoa)   1 technicolor   2 eastmancolor   3 kodachrome   4 moderni

Käyttö: python tyylit.py AINEISTOKANSIO
"""
import collections, json, re, sys, time, urllib.parse, urllib.request
from pathlib import Path

TYYLIT = ["automaattinen", "technicolor", "eastmancolor", "kodachrome", "moderni"]
OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (avoin varityssovellus)"}
# lähteen oletustyyli, jos kuvaus ei kerro filmiä
OLETUS = {"finna": 0, "finna_1950_1980": 0, "commons": 4, "fedflix": 2, "prelinger": 2, "elokuvat": 2, "kodachrome": 3}


def hae(url):
    for i in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=60) as v:
                return json.loads(v.read())
        except Exception:
            time.sleep(3 * (i + 1))
    return {}


def teksti(meta):
    osat = []
    for k in ("title", "description", "subject", "notes", "color"):
        v = meta.get(k)
        osat += v if isinstance(v, list) else [v] if v else []
    return " ".join(map(str, osat)).lower()


def luokittele(lahde, meta, vuosi):
    t = teksti(meta)
    if re.search(r"kodachrome|home movie|amateur film|family film", t):
        return 3
    if "technicolor" in t:
        return 1
    if re.search(r"eastmancolor|eastman color|deluxe|metrocolor|warnercolor|pathecolor|ektachrome|agfa|ferrania|gevacolor", t):
        return 2
    # yhdysvaltalaiset pitkät värielokuvat ennen 1955: lähes kaikki kolminegatiivi-Technicolor
    if lahde == "elokuvat" and vuosi and vuosi < 1955:
        return 1
    return OLETUS[lahde]


def main():
    kansio = Path(sys.argv[1])
    tulos = {"oletus": OLETUS, "tyylit": TYYLIT, "elokuvat": {}}
    laskuri = collections.Counter()
    for lahde in ("prelinger", "elokuvat", "fedflix"):
        rivit = [json.loads(r) for r in (kansio / lahde / f"lahteet_{lahde}.jsonl").read_text(encoding="utf-8").splitlines() if r.strip()]
        for n, r in enumerate(rivit, 1):
            meta = hae(f"https://archive.org/metadata/{urllib.parse.quote(r['id'])}").get("metadata") or {} if lahde != "fedflix" else {"title": r.get("otsikko")}
            v = re.search(r"(19\d\d)", str(r.get("vuosi") or ""))
            s = luokittele(lahde, meta, int(v.group(1)) if v else None)
            tulos["elokuvat"][f"{lahde}_{r['id'][:60]}"] = s
            laskuri[(lahde, TYYLIT[s])] += 1
            if lahde != "fedflix":
                time.sleep(0.2)
            if n % 100 == 0:
                print(f"{lahde} {n}/{len(rivit)}", flush=True)
    (kansio / "tyylit.json").write_text(json.dumps(tulos, ensure_ascii=False, indent=0), encoding="utf-8")
    for (l, s), n in sorted(laskuri.items()):
        print(f"{l:10} {s:13} {n}")


if __name__ == "__main__":
    main()
