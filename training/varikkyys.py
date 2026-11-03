"""Värikkyysmittari: mallin ennustaman värin keskimääräinen kroma validointikuvissa verrattuna oikeaan.
Suhde < 1 = malli värittää haaleammin kuin todellisuus (L1-koulutuksen tyypillinen harmaantuminen).

Käyttö: python varikkyys.py VALIDOINTI.txt PAINOT.pt [PAINOT2.pt ...] [--n 40] [--laite cpu]
"""
import argparse, sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).parent))
from jatkokouluta import Aineisto, lab2rgb, tuo_ddcolor  # noqa: E402


def main():
    p = argparse.ArgumentParser()
    p.add_argument("validointi")
    p.add_argument("painot", nargs="+")
    p.add_argument("--n", type=int, default=40)
    p.add_argument("--laite", default="cpu")
    a = p.parse_args()
    DDColor, _, _ = tuo_ddcolor()
    tiedostot = [Path(r) for r in Path(a.validointi).read_text(encoding="utf-8").splitlines()][: a.n]
    data = Aineisto(tiedostot, 384, kouluta=False)
    naytteet = [data[i] for i in range(len(data))]
    oikea = np.mean([np.hypot(ab[0], ab[1]).mean().item() for _, ab in naytteet])
    print(f"oikea keskikroma {oikea:.2f}")
    for polku in a.painot:
        m = DDColor(encoder_name="convnext-l", decoder_name="MultiScaleColorDecoder", input_size=(512, 512),
                    num_output_channels=2, last_norm="Spectral", do_normalize=False, num_queries=100, num_scales=3, dec_layers=9)
        sd = torch.load(polku, map_location="cpu", weights_only=True)
        m.load_state_dict(sd.get("params", sd))
        m.to(a.laite).eval()
        kromat, virheet = [], []
        with torch.no_grad():
            for L, ab in naytteet:
                L = L[None].to(a.laite)
                ulos = m(lab2rgb(torch.cat([L, torch.zeros(1, 2, *L.shape[2:], device=a.laite)], 1)))[0].cpu()
                kromat.append(torch.hypot(ulos[0], ulos[1]).mean().item())
                virheet.append((ulos - ab).abs().mean().item())
        k = float(np.mean(kromat))
        print(f"{Path(polku).parent.name}/{Path(polku).name}: keskikroma {k:.2f} (suhde {k / oikea:.2f}), ab-L1 {np.mean(virheet):.2f}", flush=True)


if __name__ == "__main__":
    main()
