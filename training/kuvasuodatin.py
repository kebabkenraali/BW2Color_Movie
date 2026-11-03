"""Elokuvakuvien laatusuodatin: haalistunut filmi (voimakas värivirhe), tekstiruudut ja tyhjät ruudut pois.

Käyttö kirjastona (hylkaa(rgb) -> syy tai None) tai: python kuvasuodatin.py KANSIO [--poista] [--arkki ARKKI.jpg]
"""
import argparse, sys
from pathlib import Path

import numpy as np
from PIL import Image
from skimage.color import rgb2lab

VARIVIRHE = 22.0  # koko kuvan keskimääräinen a/b-poikkeama neutraalista (Lab)
TASAINEN = 0.55   # osuus pikseleistä lähellä kuvan mediaaniväriä


def hylkaa(rgb, varivirhe=VARIVIRHE):
    """rgb: uint8 (H, W, 3). Palauttaa hylkäyksen syyn tai None."""
    im = Image.fromarray(rgb).resize((96, 72))
    lab = rgb2lab(np.asarray(im, np.float32) / 255.0).astype(np.float32)
    a, b = lab[..., 1], lab[..., 2]
    if np.hypot(a.mean(), b.mean()) > varivirhe:
        return "värivirhe"
    # tasaisuus vain valoisista kohdista: musta studiotausta ei tee kuvasta tekstiruutua
    valoisa = lab[..., 0] > 15.0
    if valoisa.mean() < 0.2:
        return "pimeä"
    vl = lab[valoisa]
    m = np.median(vl, 0)
    if (np.linalg.norm(vl - m, axis=1) < 8.0).mean() > TASAINEN:
        return "tasainen"
    return None


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kansio")
    p.add_argument("--poista", action="store_true")
    p.add_argument("--arkki")
    p.add_argument("--varivirhe", type=float, default=VARIVIRHE)
    a = p.parse_args()
    tiedostot = sorted(Path(a.kansio).glob("*.jpg"))
    hyl = []
    for f in tiedostot:
        syy = hylkaa(np.asarray(Image.open(f).convert("RGB")), a.varivirhe)
        if syy:
            hyl.append((f, syy))
    print(f"{len(hyl)} / {len(tiedostot)} hylätään:", {s: sum(1 for _, x in hyl if x == s) for s in {x for _, x in hyl}})
    if a.arkki and hyl:
        W = 8
        c = Image.new("RGB", (W * 160, ((min(len(hyl), 48) + W - 1) // W) * 120))
        for i, (f, _) in enumerate(hyl[:48]):
            im = Image.open(f).convert("RGB")
            im.thumbnail((160, 120))
            c.paste(im, ((i % W) * 160, (i // W) * 120))
        c.save(a.arkki)
    if a.poista:
        for f, _ in hyl:
            f.unlink()


if __name__ == "__main__":
    main()
