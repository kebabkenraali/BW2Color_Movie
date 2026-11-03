"""Haalistuneen värifilmin korjaus koulutusaineistoon. Vanhan elokuvafilmin syaani ja keltainen väriaine
haalistuvat, jolloin kuva vinoutuu punavioletiksi ja menettää värikkyyttä. Mallin ei pidä oppia tätä
vanhentumista vaan värit sellaisina kuin ne kuvattiin.

Korjaus lasketaan elokuvittain (kaikista saman elokuvan kuvista yhdessä, ei kuva kerrallaan, jotta
esim. auringonlaskun tai metsän oma väri säilyy): kunkin kanavan 1. ja 99. persentiili venytetään
kanavien yhteiselle tasolle. Tulos: kerroin ja siirto kanavittain (rgb * a + b, sRGB 0..1).

Käyttö: python haalistuma.py AINEISTOKANSIO [--esikatselu ARKKI.jpg]
"""
import argparse, collections, json
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

LAHTEET = ("prelinger", "fedflix", "elokuvat", "kodachrome")
VOIMA = 0.85          # osittainen korjaus: elokuvan oma väritys ei katoa kokonaan
KERROIN = (0.6, 1.7)  # rajat kanavan venytykselle
KESKI = 0.6           # keskisävyjen tasapainon voima (elokuvan oma tunnelma, esim. iltavalo, saa jäädä)
GAMMA = (0.7, 1.4)


def elokuva(polku, lahde):
    return f"{lahde}_" + polku.stem[len(lahde) + 1:].rsplit("_", 1)[0]


def pikselit(polku):
    im = Image.open(polku).convert("RGB")
    im.thumbnail((96, 96))
    return np.asarray(im, np.float32).reshape(-1, 3) / 255.0


def parametrit(polut):
    p = np.concatenate([pikselit(x) for x in polut])
    lo, hi = np.percentile(p, 1, axis=0), np.percentile(p, 99, axis=0)
    lo_t, hi_t = lo.mean(), hi.mean()
    a = np.clip((hi_t - lo_t) / np.maximum(hi - lo, 1e-3), *KERROIN)
    b = lo_t - lo * a
    # osittainen korjaus: sekoitus identiteetin kanssa
    a = 1 + VOIMA * (a - 1)
    b = VOIMA * b
    # keskisävyt: haalistuma näkyy ennen kaikkea niissä (violetti iho, vihreä taivas); kanavien
    # mediaanit lähemmäs toisiaan gammalla, joka ei siirrä mustaa eikä valkoista
    m = np.median(np.clip(p * a + b, 1e-3, 1), axis=0)
    g = np.clip(np.log(m.mean()) / np.log(m), *GAMMA)
    g = 1 + KESKI * (g - 1)
    return a.tolist(), b.tolist(), g.tolist()


def korjaa(rgb, ab):
    a, b, g = ab
    return np.clip(rgb * np.asarray(a, np.float32) + np.asarray(b, np.float32), 0, 1) ** np.asarray(g, np.float32)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("aineisto")
    p.add_argument("--esikatselu")
    a = p.parse_args()
    k = Path(a.aineisto)
    ryhmat = collections.defaultdict(list)
    for lahde in LAHTEET:
        for f in sorted((k / lahde / "kuvat").glob("*.jpg")):
            ryhmat[elokuva(f, lahde)].append(f)
    avaimet = sorted(ryhmat)
    with ProcessPoolExecutor(8) as ex:
        tulos = dict(zip(avaimet, ex.map(parametrit, [ryhmat[x] for x in avaimet], chunksize=8)))
    (k / "haalistuma.json").write_text(json.dumps(tulos), encoding="utf-8")
    poikkeama = {x: float(np.abs(np.asarray(v[0]) - 1).max() + np.abs(np.asarray(v[1])).max() + np.abs(np.asarray(v[2]) - 1).max()) for x, v in tulos.items()}
    print(f"{len(tulos)} elokuvaa, korjauksen suuruus: mediaani {np.median(list(poikkeama.values())):.3f}, "
          f"yli 0.2: {sum(v > 0.2 for v in poikkeama.values())}")
    if a.esikatselu:
        # voimakkaimmin ja keskimääräisesti korjatut: ennen | jälkeen
        jarj = sorted(poikkeama, key=poikkeama.get)
        valitut = jarj[-8:] + jarj[len(jarj) // 2:len(jarj) // 2 + 4]
        rivit = []
        for x in valitut:
            f = ryhmat[x][len(ryhmat[x]) // 2]
            rgb = np.asarray(Image.open(f).convert("RGB").resize((320, 240)), np.float32) / 255.0
            rivit.append(np.concatenate([rgb, korjaa(rgb, tulos[x])], 1))
        Image.fromarray((np.concatenate(rivit) * 255).astype(np.uint8)).save(a.esikatselu, quality=90)


if __name__ == "__main__":
    main()
