"""Vertailu oikeilla mustavalkoelokuvilla: sama kuva alkuperäisellä ja jatkokoulutetulla mallilla.
Tulos: arkki, jossa jokaisella rivillä mustavalko | alkuperäinen DDColor | uusi malli.

Käyttö: python vertaa.py --painot pytorch_model.bin --uusi paras.pt --ulos arkki.jpg --ffmpeg FFMPEG
        --video VIDEO:SEK[,SEK...] [VIDEO:SEK ...]
"""
import argparse, subprocess, sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image, ImageDraw
from skimage import color as skcolor

sys.path.insert(0, str(Path(__file__).parent))
from jatkokouluta import TYYLEJA, aja, lab2rgb, lataa_painot, lisaa_tyyli, tuo_ddcolor  # noqa: E402

KOKO = 512
LAITE = "cuda"


def malli(DDColor, polku):
    m = DDColor(encoder_name="convnext-l", decoder_name="MultiScaleColorDecoder", input_size=(KOKO, KOKO),
                num_output_channels=2, last_norm="Spectral", do_normalize=False, num_queries=100, num_scales=3, dec_layers=9)
    lisaa_tyyli(m)
    lataa_painot(m, polku)
    return m.to(LAITE).eval()


def kuva(ffmpeg, video, t):
    raw = subprocess.run([ffmpeg, "-v", "error", "-ss", str(t), "-i", video, "-frames:v", "1", "-vf", "scale=640:-2",
                          "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
    w = 640
    h = len(raw) // (3 * w)
    return np.frombuffer(raw, np.uint8)[: w * h * 3].reshape(h, w, 3).astype(np.float32) / 255.0


def varita(m, rgb, tyyli=0):
    h, w = rgb.shape[:2]
    L = skcolor.rgb2lab(rgb)[..., :1]
    x = torch.from_numpy(np.array(Image.fromarray((rgb * 255).astype(np.uint8)).convert("L").resize((KOKO, KOKO), Image.BICUBIC),
                                  np.float32) / 255.0)
    x = x[None, None].repeat(1, 3, 1, 1).to(LAITE)
    Lx = torch.from_numpy(skcolor.rgb2lab(x[0].permute(1, 2, 0).cpu().numpy())[..., :1]).permute(2, 0, 1)[None].float().to(LAITE)
    with torch.no_grad(), torch.autocast(LAITE, dtype=torch.bfloat16, enabled=LAITE == "cuda"):
        t = torch.nn.functional.one_hot(torch.tensor([tyyli]), TYYLEJA).float().to(LAITE)
        ab = aja(m, lab2rgb(torch.cat([Lx, torch.zeros(1, 2, KOKO, KOKO, device=LAITE)], 1)), t).float()
    ab = torch.nn.functional.interpolate(ab, size=(h, w), mode="bilinear")[0].permute(1, 2, 0).cpu().numpy()
    return np.clip(skcolor.lab2rgb(np.concatenate([L, ab], 2)), 0, 1)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--painot", required=True)
    p.add_argument("--uusi", required=True)
    p.add_argument("--ulos", required=True)
    p.add_argument("--ffmpeg", required=True)
    p.add_argument("--video", nargs="+", required=True)
    p.add_argument("--laite", default="cuda")
    p.add_argument("--tyylit", type=int, nargs="*", help="uusi malli näillä tyyleillä (sarake kutakin kohden)")
    a = p.parse_args()
    global LAITE
    LAITE = a.laite
    DDColor, _, _ = tuo_ddcolor()
    vanha, uusi = malli(DDColor, a.painot), malli(DDColor, a.uusi)
    rivit = []
    for v in a.video:
        polku, ajat = v.rsplit(":", 1)
        for t in ajat.split(","):
            rgb = kuva(a.ffmpeg, polku, float(t))
            harmaa = np.repeat(rgb.mean(2, keepdims=True), 3, 2)
            uudet = [varita(uusi, harmaa, s) for s in (a.tyylit or [0])]
            rivit.append(np.concatenate([harmaa, varita(vanha, harmaa), *uudet], 1))
    leveys = max(r.shape[1] for r in rivit)
    arkki = Image.fromarray((np.concatenate([np.pad(r, ((0, 0), (0, leveys - r.shape[1]), (0, 0))) for r in rivit], 0) * 255).astype(np.uint8))
    d = ImageDraw.Draw(arkki)
    nimet = ["automaattinen", "technicolor", "eastmancolor", "kodachrome", "moderni"]
    for i, teksti in enumerate(["mustavalko", "vertailumalli"] + [f"uusi: {nimet[s]}" for s in (a.tyylit or [0])]):
        d.text((i * 640 + 8, 6), teksti, fill=(255, 255, 0))
    arkki.save(a.ulos, quality=92)
    print(a.ulos)


if __name__ == "__main__":
    main()
