"""Wikimedia Commons: CC0-lisensoidut laatukuvat (Quality images), vain valokuvat (kameran tiedot
metatiedoissa) ja kuvausvuosi 2001 tai myöhemmin – modernimpi väripaletti aikakauden aineiston rinnalle.
Lähteet kirjataan tiedostoon lahteet_commons.jsonl.

Käyttö: python commons_lataa.py KOHDEKANSIO [--enintaan 4000]
"""
import argparse, io, json, re, sys, time, urllib.parse, urllib.request
from pathlib import Path

from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))
from finna_lataa import varikuva  # noqa: E402

API = "https://commons.wikimedia.org/w/api.php"
OTSAKE = {"User-Agent": "Varita-koulutusaineisto/1.0 (open-source film colorization; contact via project repository)"}
HAKU = "incategory:Quality_images haswbstatement:P275=Q6938433 filetype:bitmap"


def hae(url, yrityksia=5):
    for i in range(yrityksia):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=OTSAKE), timeout=60) as v:
                return v.read()
        except Exception:
            if i == yrityksia - 1:
                raise
            time.sleep(5 * (i + 1))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kohde")
    p.add_argument("--enintaan", type=int, default=4000)
    a = p.parse_args()
    kohde = Path(a.kohde)
    (kohde / "kuvat").mkdir(parents=True, exist_ok=True)
    luettelo = kohde / "lahteet_commons.jsonl"
    tehdyt = set()
    if luettelo.exists():
        tehdyt = {json.loads(r)["id"] for r in luettelo.read_text(encoding="utf-8").splitlines() if r.strip()}
    ok, hyl = len(tehdyt), 0
    jatko = {}
    while ok < a.enintaan:
        q = {"action": "query", "format": "json", "generator": "search", "gsrnamespace": "6", "gsrlimit": "50",
             "gsrsearch": HAKU, "prop": "imageinfo", "iiprop": "url|extmetadata|metadata", "iiurlwidth": "1024"}
        q.update(jatko)
        d = json.loads(hae(API + "?" + urllib.parse.urlencode(q)))
        for sivu in (d.get("query", {}).get("pages") or {}).values():
            otsikko = sivu["title"]
            if otsikko in tehdyt or not sivu.get("imageinfo"):
                continue
            ii = sivu["imageinfo"][0]
            em = {k: v.get("value") for k, v in (ii.get("extmetadata") or {}).items()}
            meta = {m["name"]: m["value"] for m in (ii.get("metadata") or []) if isinstance(m, dict) and "name" in m}
            lisenssi = str(em.get("LicenseShortName", ""))
            vuosi = re.search(r"(19|20)\d\d", str(meta.get("DateTimeOriginal") or em.get("DateTimeOriginal") or ""))
            if "CC0" not in lisenssi.upper() or "Model" not in meta or not vuosi or int(vuosi.group()) < 2001:
                hyl += 1
                continue
            try:
                im = Image.open(io.BytesIO(hae(ii["thumburl"])))
                hyva, m, _ = varikuva(im)
            except Exception:
                continue
            if not hyva:
                hyl += 1
                continue
            nimi = "commons_" + re.sub(r"[^A-Za-z0-9]", "_", otsikko[5:])[:80] + ".jpg"
            im.convert("RGB").save(kohde / "kuvat" / nimi, quality=95)
            tekija = re.sub(r"<[^>]+>", "", str(em.get("Artist") or "")).strip()
            tieto = {"id": otsikko, "tiedosto": nimi, "otsikko": otsikko[5:], "vuosi": vuosi.group(), "tekija": tekija,
                     "lisenssi": lisenssi, "lisenssi_url": em.get("LicenseUrl"), "lahde": ii.get("descriptionurl"),
                     "varikkyys": round(m, 1)}
            luettelo.open("a", encoding="utf-8").write(json.dumps(tieto, ensure_ascii=False) + "\n")
            tehdyt.add(otsikko)
            ok += 1
            time.sleep(0.2)
        print(f"hyväksytty {ok}, hylätty {hyl}", flush=True)
        if "continue" not in d:
            break
        jatko = d["continue"]
    print(f"Valmis: {ok} kuvaa", flush=True)


if __name__ == "__main__":
    main()
