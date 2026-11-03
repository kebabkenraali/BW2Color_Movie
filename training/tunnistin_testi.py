"""Tunnistimen testi oikeilla väritetyillä videoilla: kuva | tunnistimen löydökset magentana.

Käyttö: python tunnistin_testi.py PAINOT.pt ULOS.jpg FFMPEG VIDEO:SEK,SEK [VIDEO:SEK ...] [--n 200]
        --etsi N: käy läpi N tasavälistä kuvaa ja näytä ne, joissa löydöksiä on eniten
"""
import argparse, subprocess, sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image
from skimage import color

sys.path.insert(0, str(Path(__file__).parent))
from tunnistin_kouluta import UNet  # noqa: E402


def kuva(ffmpeg, video, t, w=512):
    raw = subprocess.run([ffmpeg, "-v", "error", "-ss", str(t), "-i", video, "-frames:v", "1", "-vf", f"scale={w}:-2",
                          "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
    h = len(raw) // (3 * w)
    return np.frombuffer(raw, np.uint8)[: w * h * 3].reshape(h, w, 3).astype(np.float32) / 255.0


def kesto(ffmpeg, video):
    ffprobe = str(Path(ffmpeg).with_name("ffprobe.exe"))
    return float(subprocess.run([ffprobe, "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", video],
                                capture_output=True, text=True).stdout.strip())


def maski(m, rgb):
    lab = color.rgb2lab(rgb)
    x = np.concatenate([lab[..., :1] / 100, lab[..., 1:] / 100], -1).transpose(2, 0, 1)
    h, w = rgb.shape[:2]
    # peilattu reuna (16 px) joka suuntaan: verkon reunat eivät tuota vääriä löydöksiä
    R = 16
    H, W = (h + 2 * R + 15) // 16 * 16, (w + 2 * R + 15) // 16 * 16
    x = np.pad(x, ((0, 0), (R, H - h - R), (R, W - w - R)), mode="reflect")
    with torch.no_grad():
        mk = torch.sigmoid(m(torch.from_numpy(x[None]).float().cuda()))[0, 0, R:R + h, R:R + w].cpu().numpy()
    mk = (mk > 0.5) & (lab[..., 0] > 15.0)  # hyvin tummista kohdista ei voi sanoa mitään
    # kuten Väritässä: reunaan ulottuvat ja hyvin pienet alueet hylätään
    from scipy import ndimage
    nim, n = ndimage.label(mk, structure=np.ones((3, 3)))
    for k in range(1, n + 1):
        alue = nim == k
        ys, xs = np.nonzero(alue)
        if ys.min() == 0 or xs.min() == 0 or ys.max() == h - 1 or xs.max() == w - 1 or alue.sum() < 0.002 * h * w:
            mk[alue] = False
    return mk.astype(np.float32)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("painot")
    p.add_argument("ulos")
    p.add_argument("ffmpeg")
    p.add_argument("videot", nargs="+")
    p.add_argument("--etsi", type=int, default=0)
    a = p.parse_args()
    m = UNet().cuda().eval()
    m.load_state_dict(torch.load(a.painot, map_location="cuda", weights_only=True))
    rivit = []
    for v in a.videot:
        polku, _, ajat = v.rpartition(":") if ":" in v[3:] else (v, "", "")
        if a.etsi:
            k = kesto(a.ffmpeg, polku)
            ehdokkaat = []
            for t in np.linspace(30, k - 30, a.etsi):
                rgb = kuva(a.ffmpeg, polku, t, 384)
                ehdokkaat.append(((maski(m, rgb) > 0.5).mean(), t))
            ehdokkaat.sort(reverse=True)
            ajat_l = [t for _, t in ehdokkaat[:4]]
            print(polku, "eniten löydöksiä:", [f"{t:.0f}s {o:.3f}" for o, t in ehdokkaat[:4]])
        else:
            ajat_l = [float(t) for t in ajat.split(",")]
        for t in ajat_l:
            rgb = kuva(a.ffmpeg, polku, t)
            mk = maski(m, rgb)[..., None]
            ov = rgb * (1 - 0.6 * (mk > 0.5)) + np.array([1, 0, 1]) * 0.6 * (mk > 0.5)
            rivit.append(np.concatenate([rgb, ov], 1))
    w = max(r.shape[1] for r in rivit)
    Image.fromarray((np.concatenate([np.pad(r, ((0, 0), (0, w - r.shape[1]), (0, 0))) for r in rivit]) * 255).astype(np.uint8)).save(a.ulos, quality=90)


if __name__ == "__main__":
    main()
