"""Koulutusaineiston lähdeluettelo julkaisua varten (tekijämaininnat lisenssien mukaisesti), englanniksi.

Käyttö: python lahdeluettelo.py AINEISTOKANSIO ULOS.txt
"""
import collections, json, sys
from pathlib import Path


def lue(p):
    return [json.loads(r) for r in Path(p).read_text(encoding="utf-8").splitlines() if r.strip()] if Path(p).exists() else []


def main():
    kansio, ulos = Path(sys.argv[1]), Path(sys.argv[2])
    finna = lue(kansio / "finna" / "lahteet_finna.jsonl") + lue(kansio / "finna_1950_1980" / "lahteet_finna.jsonl")
    prel = lue(kansio / "prelinger" / "lahteet_prelinger.jsonl")
    commons = lue(kansio / "commons" / "lahteet_commons.jsonl")
    fedflix = lue(kansio / "fedflix" / "lahteet_fedflix.jsonl")
    elokuvat = lue(kansio / "elokuvat" / "lahteet_elokuvat.jsonl")
    koda = [r for r in lue(kansio / "kodachrome" / "lahteet_kodachrome.jsonl") if r.get("kuvia")]
    rivit = [
        "BW2COLOR MOVIE – TRAINING DATA OF THE FINE-TUNED COLORIZATION MODEL (ddcolor_suomi)",
        "=" * 80,
        "",
        "The model ddcolor_suomi is a fine-tuned version of DDColor (piddnad/DDColor, Apache-2.0).",
        "Only freely licensed material was used: public domain (PDM), CC0 and works of the US federal government.",
        "The images were converted to simulated black-and-white film for training; the model weights do not",
        "contain the images as such. This list covers all downloaded material.",
        "",
        f"1. FINNA.FI – {len(finna)} photographs (1940–2000)",
        "-" * 80,
    ]
    lisenssit = collections.Counter(f["lisenssi"] for f in finna)
    rivit.append("Licences: " + ", ".join(f"{k} {v}" for k, v in lisenssit.most_common()))
    arkistot = collections.Counter(f["arkisto"] for f in finna)
    rivit.append("Archives: " + ", ".join(f"{k} ({v})" for k, v in arkistot.most_common()))
    rivit.append("")
    for f in sorted(finna, key=lambda x: (x.get("arkisto") or "", x.get("vuosi") or "", x["id"])):
        tekija = ", ".join(t for t in f.get("tekija") or [] if t) or "author unknown"
        rivit.append(f"{f.get('otsikko') or '(untitled)'} ({f.get('vuosi') or 'year ?'}). {tekija}. "
                     f"{f.get('arkisto')}. {f.get('lisenssi')}. {f['lahde']}")
    rivit += ["", f"2. PRELINGER ARCHIVES (Internet Archive) – {len(prel)} films, public domain", "-" * 80]
    for p in sorted(prel, key=lambda x: (x.get("vuosi") or "", x["id"])):
        rivit.append(f"{p.get('otsikko')} ({p.get('vuosi')}). {p.get('tekija') or ''}. Public Domain. {p['lahde']}")
    rivit += ["", f"3. PRELINGER ARCHIVES, AMATEUR AND KODACHROME FILMS – {len(koda)} films, public domain", "-" * 80]
    for p in sorted(koda, key=lambda x: (str(x.get("vuosi") or ""), x["id"])):
        rivit.append(f"{p.get('otsikko')} ({p.get('vuosi')}). {p.get('tekija') or ''}. Public Domain. {p['lahde']}")
    rivit += ["", f"4. WIKIMEDIA COMMONS – {len(commons)} photographs (2001–), CC0, Quality images", "-" * 80]
    for c in sorted(commons, key=lambda x: x["id"]):
        rivit.append(f"{c.get('otsikko')} ({c.get('vuosi')}). {c.get('tekija') or 'author unknown'}. "
                     f"{c.get('lisenssi')}. {c['lahde']}")
    rivit += ["", f"5. FEDFLIX (Internet Archive) – {len(fedflix)} films of US federal agencies, 1950–1980",
              "Works of the US federal government are not subject to copyright under US law (17 U.S.C. § 105).", "-" * 80]
    for e in sorted(fedflix, key=lambda x: (str(x.get("vuosi") or ""), x["id"])):
        rivit.append(f"{e.get('otsikko')} ({e.get('vuosi')}). {e.get('tekija') or ''}. {e['lahde']}")
    rivit += ["", f"6. INTERNET ARCHIVE, FEATURE FILMS – {len(elokuvat)} US colour feature films, 1950–1980",
              "Marked public domain on the Internet Archive. The public-domain status is based on US law (e.g. expired",
              "or unrenewed copyright); it may differ in other countries. Foreign productions were excluded, because the",
              "URAA (1996) restored US copyright in many of them.", "-" * 80]
    for e in sorted(elokuvat, key=lambda x: (str(x.get("vuosi") or ""), x["id"])):
        rivit.append(f"{e.get('otsikko')} ({e.get('vuosi')}). {e.get('tekija') or ''}. {e['lahde']}")
    ulos.write_text("\n".join(rivit) + "\n", encoding="utf-8")
    print(f"{ulos}: {len(finna)} + {len(commons)} images, {len(prel)} + {len(koda)} + {len(fedflix)} + {len(elokuvat)} films")


if __name__ == "__main__":
    main()
