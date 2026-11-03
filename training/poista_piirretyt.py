"""Poistaa piirroselokuvien kuvat elokuva-aineistosta: kuva luokitellaan DINOv2-piirteillä ja käsin
merkityillä esimerkeillä (piirretyt.py), ja päätös tehdään elokuvittain – jos yli puolet elokuvan
kuvista on piirroksia, kaikki sen kuvat poistetaan (yksittäiset väärät osumat eivät vie filmikuvia).

Käyttö: python poista_piirretyt.py ELOKUVAKANSIO ESIMERKIT.json ETULIITE
"""
import collections, json, sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
from piirretyt import Piirteet, luokittele  # noqa: E402


def main():
    k, esim, etuliite = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    e = json.loads(esim.read_text(encoding="utf-8"))
    # esimerkkien piirteet talteen .npz:hen: esimerkkikuvat voivat poistua (piirroselokuvien poisto)
    e = {k: [x for x in v if Path(x).exists()] for k, v in e.items()}
    pf = Piirteet()
    tallenne = esim.with_suffix(".npz")
    vanha = np.load(tallenne) if tallenne.exists() else None
    p = np.concatenate([x for x in [vanha["piirros"] if vanha is not None else None, pf(e["piirros"]) if e["piirros"] else None] if x is not None])
    f = np.concatenate([x for x in [vanha["filmi"] if vanha is not None else None, pf(e["filmi"]) if e["filmi"] else None] if x is not None])
    np.savez(tallenne, piirros=p, filmi=f)
    print(f"esimerkkejä: {len(p)} piirrosta, {len(f)} filmiä")
    t = sorted((k / "kuvat").glob("*.jpg"))
    o = luokittele(pf(t), p, f)
    ryhma = collections.defaultdict(list)
    for x, v in zip(t, o):
        ryhma[x.stem[len(etuliite) + 1:].rsplit("_", 1)[0]].append((x, v))
    pois = [g for g, v in ryhma.items() if np.mean([b > 0.5 for _, b in v]) > 0.5]
    n = 0
    for g in pois:
        for x, _ in ryhma[g]:
            x.unlink()
            n += 1
    vanhat = set((k / "piirretyt.txt").read_text(encoding="utf-8").split()) if (k / "piirretyt.txt").exists() else set()
    (k / "piirretyt.txt").write_text("\n".join(sorted(vanhat | set(pois))), encoding="utf-8")
    print(f"elokuvia {len(ryhma)}, piirroselokuviksi {len(pois)}, poistettu {n} kuvaa, jäljellä {len(t) - n}")


if __name__ == "__main__":
    main()
