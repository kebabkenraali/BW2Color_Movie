"""Tunnistimen koe oikeilla mustavalkoelokuvilla: N tasavälistä kuvaa väritetään DDColorilla, tunnistin
ajetaan tulokselle ja näytetään kuvat, joissa löydöksiä on eniten (mustavalko | väritys | löydökset).

Käyttö: python tunnistin_etsi.py DDCOLOR.pt TUNNISTIN.pt ULOS.jpg FFMPEG VIDEO [VIDEO ...] [--n 80] [--naytä 8]
"""
import argparse, sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))
from tunnistin_kouluta import UNet  # noqa: E402
from tunnistin_testi import kesto, kuva, maski  # noqa: E402
import vertaa  # noqa: E402


def main():
    p = argparse.ArgumentParser()
    p.add_argument("ddcolor")
    p.add_argument("tunnistin")
    p.add_argument("ulos")
    p.add_argument("ffmpeg")
    p.add_argument("videot", nargs="+")
    p.add_argument("--n", type=int, default=80)
    p.add_argument("--nayta", type=int, default=8)
    a = p.parse_args()
    DDColor, _, _ = vertaa.tuo_ddcolor()
    dd = vertaa.malli(DDColor, a.ddcolor)
    t = UNet().cuda().eval()
    t.load_state_dict(torch.load(a.tunnistin, map_location="cuda", weights_only=True))
    tulokset = []
    for v in a.videot:
        k = kesto(a.ffmpeg, v)
        for aika in np.linspace(60, k - 60, a.n):
            rgb = kuva(a.ffmpeg, v, aika, 512)
            harmaa = np.repeat(rgb.mean(2, keepdims=True), 3, 2)
            vari = vertaa.varita(dd, harmaa)
            mk = maski(t, vari) > 0.5
            tulokset.append((mk.mean(), Path(v).name, aika, harmaa, vari, mk))
    tulokset.sort(key=lambda x: -x[0])
    print("löydösten osuus kuvista:", np.mean([x[0] > 0.01 for x in tulokset]).round(3))
    rivit = []
    for o, nimi, aika, harmaa, vari, mk in tulokset[: a.nayta]:
        print(f"{nimi} {aika:.0f}s: {o:.3f}")
        ov = vari * (1 - 0.6 * mk[..., None]) + np.array([1, 0, 1]) * 0.6 * mk[..., None]
        rivit.append(np.concatenate([harmaa, vari, ov], 1))
    w = max(r.shape[1] for r in rivit)
    Image.fromarray((np.clip(np.concatenate([np.pad(r, ((0, 0), (0, w - r.shape[1]), (0, 0))) for r in rivit]), 0, 1) * 255)
                    .astype(np.uint8)).save(a.ulos, quality=88)


if __name__ == "__main__":
    main()
