"""Piirroskuvien tunnistus DINOv2-piirteillä (koko kuvan CLS) ja käsin merkityillä esimerkeillä:
lähimmät naapurit. Elokuva-aineistosta poistetaan piirretyt, jotta malli ei opi piirrosvärejä.

Käyttö: python piirretyt.py esimerkit.json KANSIO [--poista] [--arkki ARKKI.jpg]
  esimerkit.json: {"piirros": [tiedostot], "filmi": [tiedostot]}
"""
import argparse, json, sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
from PIL import Image

DINO = r"C:/BW2COLOR/Varita/models/dinov2_small/dinov2_small.onnx"


class Piirteet:
    def __init__(self):
        self.s = ort.InferenceSession(DINO, providers=["CPUExecutionProvider"])

    def __call__(self, polut, era=16):
        ulos = []
        for i in range(0, len(polut), era):
            x = np.stack([self.syote(p) for p in polut[i:i + era]])
            cls = self.s.run(None, {"pixel_values": x})[0][:, 0]
            ulos.append(cls / np.linalg.norm(cls, axis=1, keepdims=True))
        return np.concatenate(ulos)

    @staticmethod
    def syote(p):
        a = np.asarray(Image.open(p).convert("RGB").resize((224, 224), Image.BICUBIC), np.float32) / 255.0
        a = (a - [0.485, 0.456, 0.406]) / [0.229, 0.224, 0.225]
        return a.transpose(2, 0, 1).astype(np.float32)


def luokittele(f, piirros, filmi, k=5):
    """Osuus k lähimmästä esimerkistä, jotka ovat piirroksia."""
    kaikki = np.concatenate([piirros, filmi])
    tunnus = np.array([1] * len(piirros) + [0] * len(filmi))
    s = f @ kaikki.T
    lahin = np.argsort(-s, axis=1)[:, :k]
    return tunnus[lahin].mean(1)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("esimerkit")
    p.add_argument("kansio")
    p.add_argument("--poista", action="store_true")
    p.add_argument("--arkki")
    a = p.parse_args()
    e = json.loads(Path(a.esimerkit).read_text(encoding="utf-8"))
    pf = Piirteet()
    piirros, filmi = pf(e["piirros"]), pf(e["filmi"])
    # jätä-yksi-pois -tarkkuus esimerkeillä
    kaikki = np.concatenate([piirros, filmi])
    tunnus = np.array([1] * len(piirros) + [0] * len(filmi))
    oikein = 0
    for i in range(len(kaikki)):
        muut = np.delete(np.arange(len(kaikki)), i)
        s = kaikki[i] @ kaikki[muut].T
        lahin = muut[np.argsort(-s)[:5]]
        oikein += int((tunnus[lahin].mean() > 0.5) == tunnus[i])
    print(f"esimerkit: {len(piirros)} piirrosta, {len(filmi)} filmiä, jätä-yksi-pois {oikein}/{len(kaikki)}")
    tiedostot = sorted(Path(a.kansio).glob("*.jpg"))
    f = pf(tiedostot)
    osuus = luokittele(f, piirros, filmi)
    piirretyt = [t for t, o in zip(tiedostot, osuus) if o > 0.5]
    print(f"piirroksiksi luokiteltu {len(piirretyt)} / {len(tiedostot)}")
    if a.arkki and piirretyt:
        W = 8
        valinta = piirretyt[:: max(1, len(piirretyt) // 48)][:48]
        c = Image.new("RGB", (W * 160, ((len(valinta) + W - 1) // W) * 120))
        for i, t in enumerate(valinta):
            im = Image.open(t).convert("RGB")
            im.thumbnail((160, 120))
            c.paste(im, ((i % W) * 160, (i // W) * 120))
        c.save(a.arkki)
    if a.poista:
        for t in piirretyt:
            t.unlink()


if __name__ == "__main__":
    main()
