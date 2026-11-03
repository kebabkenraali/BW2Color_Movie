"""Värittämättömien kohtien tunnistin: pieni U-Net, joka saa väritetyn kuvan (L, a, b) ja antaa maskin
kohdista, jotka olisi pitänyt värittää mutta jäivät harmaiksi (kasvot, ihminen tai esine).

Merkinnät (aineisto: tunnistin_aineisto.py, DDColorin väritykset ja oikeat värit):
  1. synteettiset: väritetystä kuvasta harmaannetaan kokonaisia esineitä (SLIC-superpikselien ryhmät)
  2. synteettiset kasvot: väritetyn kuvan ihoalue harmaannetaan
Luonnostaan harmaat alueet (betoni, savu, valkoinen paita), hyvin tummat alueet ja mustat reunapalkit
ovat negatiivisia esimerkkejä. (DDColorin "todelliset" virheet oikeaan kuvaan verrattuna osoittautuivat
enimmäkseen kohinaksi ja uskottaviksi vaihtoehtoisiksi väreiksi, joten niitä ei käytetä.)
Hyvin tummia kohtia (L < TUMMA) ei merkitä: niiden väriä ei voi arvioida.

Käyttö: python tunnistin_kouluta.py --aineisto aineisto/tunnistin --ulos ajot/tunnistin1 [--askeleet 8000]
"""
import argparse, glob, json, random, time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from skimage.segmentation import slic

KOKO = 256
TUMMA = 15.0


def ihomaski(L, ab):
    savy = (np.degrees(np.arctan2(ab[1], ab[0])) + 360) % 360
    k = np.hypot(ab[0], ab[1])
    return (savy > 20) & (savy < 80) & (k > 8) & (k < 55) & (L > 20) & (L < 95)


def naapurit(seg):
    n = seg.max() + 1
    vieru = [set() for _ in range(n)]
    for a, b in ((seg[:, :-1], seg[:, 1:]), (seg[:-1, :], seg[1:, :])):
        ero = a != b
        for x, y in zip(a[ero].ravel(), b[ero].ravel()):
            vieru[x].add(y)
            vieru[y].add(x)
    return vieru


def kasvata(seg, vieru, varit, ala, siemen, max_osuus, r):
    """Alue superpikselistä `siemen`: liitetään väriltään samankaltaisia naapureita, kunnes koko on
    enintään `max_osuus` kuvasta – suunnilleen yhden esineen tai taustapinnan kokoinen."""
    kaikki = ala.sum()
    alue, reuna, koko = {siemen}, [siemen], ala[siemen]
    raja = r.uniform(8, 18)
    while reuna:
        nyt = reuna.pop(0)
        for n in vieru[nyt]:
            if n in alue or np.linalg.norm(varit[n] - varit[siemen]) > raja:
                continue
            if (koko + ala[n]) / kaikki > max_osuus:
                continue
            alue.add(n)
            reuna.append(n)
            koko += ala[n]
    return np.isin(seg, list(alue))


class Aineisto(torch.utils.data.Dataset):
    def __init__(self, tiedostot, kouluta=True):
        self.t, self.kouluta = tiedostot, kouluta

    def __len__(self):
        return len(self.t)

    def __getitem__(self, i):
        r = random.Random() if self.kouluta else random.Random(i)
        d = np.load(self.t[i])
        L = d["L"].astype(np.float32) / 2.5
        gt = d["ab"].astype(np.float32)
        ab = d["ennuste"].astype(np.float32)
        k_gt, k = np.hypot(gt[0], gt[1]), np.hypot(ab[0], ab[1])
        maski = np.zeros(L.shape, bool)
        # 1. synteettiset esineet
        if r.random() < 0.7:
            rgb = np.clip(np.stack([L] * 3, -1) / 100, 0, 1)
            seg = slic(np.concatenate([rgb[..., :1], (gt.transpose(1, 2, 0) + 128) / 255], -1), n_segments=r.choice([60, 120, 250]),
                       compactness=10, channel_axis=-1, start_label=0)
            n = seg.max() + 1
            ala = np.bincount(seg.ravel(), minlength=n).astype(np.float32)
            varit = np.stack([np.bincount(seg.ravel(), weights=c.ravel(), minlength=n) / np.maximum(ala, 1)
                              for c in (L, gt[0], gt[1])], 1)
            vk = np.bincount(seg.ravel(), weights=k.ravel(), minlength=n) / np.maximum(ala, 1)
            reunalla = np.zeros(n, bool)
            reunalla[np.unique(np.concatenate([seg[0], seg[-1], seg[:, 0], seg[:, -1]]))] = True
            vieru = naapurit(seg)
            # esineet: eivät kosketa reunaa, enintään 10 % kuvasta, väritetty ja riittävän valoisa
            ehdokkaat = [s for s in range(n) if not reunalla[s] and vk[s] > 6 and varit[s, 0] > TUMMA + 10]
            for s in r.sample(ehdokkaat, min(len(ehdokkaat), r.randint(1, 3))):
                alue = kasvata(seg, vieru, varit, ala, s, r.uniform(0.01, 0.10), r) & ~reunalla[seg]
                ab[:, alue] *= r.uniform(0.0, 0.4)
                maski |= alue
            # taustat (taivas, seinä, katu): harmaannetaan ilman merkintää – harmaa tausta on sallittu
            if r.random() < 0.5:
                tausta = [s for s in range(n) if reunalla[s]]
                if tausta:
                    alue = kasvata(seg, vieru, varit, ala, r.choice(tausta), r.uniform(0.15, 0.5), r)
                    ab[:, alue & ~maski] *= r.uniform(0.0, 0.4)
        # 2. synteettiset kasvot / iho
        if r.random() < 0.3:
            iho = ihomaski(L, ab) & (k > 6)
            if iho.mean() > 0.005:
                ab[:, iho] *= r.uniform(0.0, 0.3)
                maski |= iho
        maski &= L > TUMMA
        # mustat reunapalkit (letterbox / pillarbox): eivät ole värittämättömiä kohtia
        if r.random() < 0.25:
            h, w = L.shape
            leveys = r.randint(8, 70)
            if r.random() < 0.5:
                alue = (slice(None), slice(0, leveys)) if r.random() < 0.5 else (slice(None), slice(w - leveys, w))
            else:
                alue = (slice(0, leveys), slice(None)) if r.random() < 0.5 else (slice(h - leveys, h), slice(None))
            L[alue] = r.uniform(0, 4)
            ab[(slice(None),) + alue] = 0
            maski[alue] = False
        # koko kuvan värikkyys ja sävy vaihtelevat: tunnistin ei saa riippua niistä
        if self.kouluta:
            ab *= r.uniform(0.7, 1.3)
            kulma = np.radians(r.uniform(-15, 15))
            c, s = np.cos(kulma), np.sin(kulma)
            ab = np.stack([c * ab[0] - s * ab[1], s * ab[0] + c * ab[1]])
        x = np.concatenate([L[None] / 100, ab / 100]).astype(np.float32)
        y = maski[None].astype(np.float32)
        h, w = L.shape
        if self.kouluta:
            y0, x0 = r.randint(0, h - KOKO), r.randint(0, w - KOKO)
            x, y = x[:, y0:y0 + KOKO, x0:x0 + KOKO], y[:, y0:y0 + KOKO, x0:x0 + KOKO]
            if r.random() < 0.5:
                x, y = x[..., ::-1], y[..., ::-1]
        return torch.from_numpy(np.ascontiguousarray(x)), torch.from_numpy(np.ascontiguousarray(y))


def lohko(a, b):
    return nn.Sequential(nn.Conv2d(a, b, 3, padding=1), nn.BatchNorm2d(b), nn.ReLU(inplace=True),
                         nn.Conv2d(b, b, 3, padding=1), nn.BatchNorm2d(b), nn.ReLU(inplace=True))


class UNet(nn.Module):
    def __init__(self, k=(24, 48, 96, 192, 256)):
        super().__init__()
        self.alas = nn.ModuleList([lohko(3, k[0])] + [lohko(k[i], k[i + 1]) for i in range(len(k) - 1)])
        self.ylos = nn.ModuleList([lohko(k[i + 1] + k[i], k[i]) for i in reversed(range(len(k) - 1))])
        self.ulos = nn.Conv2d(k[0], 1, 1)

    def forward(self, x):
        valit = []
        for i, l in enumerate(self.alas):
            x = l(x if i == 0 else F.max_pool2d(x, 2))
            valit.append(x)
        for l, v in zip(self.ylos, reversed(valit[:-1])):
            x = l(torch.cat([F.interpolate(x, size=v.shape[2:], mode="bilinear", align_corners=False), v], 1))
        return self.ulos(x)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--aineisto", required=True)
    p.add_argument("--ulos", required=True)
    p.add_argument("--askeleet", type=int, default=8000)
    p.add_argument("--era", type=int, default=16)
    a = p.parse_args()
    ulos = Path(a.ulos)
    ulos.mkdir(parents=True, exist_ok=True)
    t = sorted(glob.glob(str(Path(a.aineisto) / "*.npz")))
    val, koul = t[-300:], t[:-300]
    m = UNet().cuda()
    print(f"parametreja {sum(q.numel() for q in m.parameters()) / 1e6:.1f} M, koulutus {len(koul)}, validointi {len(val)}", flush=True)
    opt = torch.optim.AdamW(m.parameters(), lr=1e-3, weight_decay=1e-4)
    aikataulu = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=1e-3, total_steps=a.askeleet)
    lataaja = torch.utils.data.DataLoader(Aineisto(koul), batch_size=a.era, shuffle=True, num_workers=6, drop_last=True,
                                          persistent_workers=True)
    val_lataaja = torch.utils.data.DataLoader(Aineisto(val, kouluta=False), batch_size=8, num_workers=4)

    def validoi():
        m.eval()
        tp = fp = fn = 0.0
        with torch.no_grad():
            for x, y in val_lataaja:
                e = torch.sigmoid(m(x.cuda())) > 0.5
                y = y.cuda() > 0.5
                tp += (e & y).sum().item()
                fp += (e & ~y).sum().item()
                fn += (~e & y).sum().item()
        m.train()
        tark, katt = tp / max(tp + fp, 1), tp / max(tp + fn, 1)
        return tark, katt, tp / max(tp + fp + fn, 1)

    askel, t0, paras = 0, time.time(), 0.0
    loki = open(ulos / "loki.jsonl", "a", encoding="utf-8")
    while askel < a.askeleet:
        for x, y in lataaja:
            x, y = x.cuda(non_blocking=True), y.cuda(non_blocking=True)
            with torch.autocast("cuda", dtype=torch.bfloat16):
                z = m(x).float()
            bce = F.binary_cross_entropy_with_logits(z, y, pos_weight=torch.tensor(3.0, device="cuda"))
            s = torch.sigmoid(z)
            dice = 1 - (2 * (s * y).sum() + 1) / (s.sum() + y.sum() + 1)
            loss = bce + dice
            opt.zero_grad(set_to_none=True)
            loss.backward()
            opt.step()
            aikataulu.step()
            askel += 1
            if askel % 100 == 0:
                print(f"askel {askel}: häviö {loss.item():.3f}  {(time.time() - t0) / askel:.2f} s/askel", flush=True)
            if askel % 1000 == 0 or askel == a.askeleet:
                tark, katt, iou = validoi()
                print(f"askel {askel}: validointi tarkkuus {tark:.2f}, kattavuus {katt:.2f}, IoU {iou:.2f}", flush=True)
                loki.write(json.dumps({"askel": askel, "tarkkuus": tark, "kattavuus": katt, "iou": iou}) + "\n")
                loki.flush()
                if iou > paras:
                    paras = iou
                    torch.save(m.state_dict(), ulos / "paras.pt")
            if askel >= a.askeleet:
                break


if __name__ == "__main__":
    main()
