"""Värittämättömien kohtien tunnistimen aineisto: DDColorin omat, todelliset epäonnistumiset.
Värikuvasta simuloidaan mustavalkofilmi, joka väritetään DDColorilla. Talteen: kirkkaus, oikea a/b ja
mallin a/b (384x384, kokonaisluvuiksi pakattuna). Tunnistimen koulutus päättelee niistä, missä oikea
kuva on värillinen mutta malli jätti sen harmaaksi.

Käyttö: python tunnistin_aineisto.py --aineisto KANSIO [...] --painot pytorch_model.bin --ulos KANSIO
        [--n 5000] [--ohita validointi.txt]
"""
import argparse, random, sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).parent))
from jatkokouluta import Aineisto, lab2rgb, tiedostot, tuo_ddcolor  # noqa: E402


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--aineisto", nargs="+", required=True)
    p.add_argument("--painot", required=True)
    p.add_argument("--ulos", required=True)
    p.add_argument("--n", type=int, default=5000)
    p.add_argument("--ohita", default=None, help="validointikuvat, joita ei käytetä")
    a = p.parse_args()
    ulos = Path(a.ulos)
    ulos.mkdir(parents=True, exist_ok=True)
    ohita = set(Path(a.ohita).read_text(encoding="utf-8").splitlines()) if a.ohita else set()
    kaikki = [f for f in tiedostot(a.aineisto) if str(f) not in ohita]
    random.Random(1).shuffle(kaikki)
    valinta = kaikki[: a.n]
    DDColor, _, _ = tuo_ddcolor()
    m = DDColor(encoder_name="convnext-l", decoder_name="MultiScaleColorDecoder", input_size=(512, 512),
                num_output_channels=2, last_norm="Spectral", do_normalize=False, num_queries=100, num_scales=3, dec_layers=9)
    sd = torch.load(a.painot, map_location="cpu", weights_only=True)
    m.load_state_dict({k: v.float() if v.is_floating_point() else v for k, v in sd.get("params", sd).items()})
    m.cuda().eval()
    # kouluta=True: satunnainen rajaus, varjot, savu ja filmin simulointi kuten koulutuksessa
    data = Aineisto(valinta, 384, kouluta=True)
    lataaja = torch.utils.data.DataLoader(data, batch_size=8, num_workers=6)
    n = 0
    with torch.no_grad(), torch.autocast("cuda", dtype=torch.bfloat16):
        for L, ab in lataaja:
            x = lab2rgb(torch.cat([L, torch.zeros_like(ab)], 1).cuda())
            ennuste = m(x).float().cpu()
            for j in range(len(L)):
                np.savez_compressed(ulos / f"{n:05d}.npz", L=np.round(L[j, 0].numpy() * 2.5).astype(np.uint8),
                                    ab=np.clip(np.round(ab[j].numpy()), -127, 127).astype(np.int8),
                                    ennuste=np.clip(np.round(ennuste[j].numpy()), -127, 127).astype(np.int8))
                n += 1
            if n % 400 < 8:
                print(f"{n}/{len(valinta)}", flush=True)
    print(f"valmis: {n} näytettä -> {ulos}")


if __name__ == "__main__":
    main()
