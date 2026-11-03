"""DDColorin jatkokoulutus vapaasti lisensoidulla aikakauden väriaineistolla (Finna, Prelinger).

Syöte: värikuvasta simuloitu mustavalkofilmi (pankromaattinen herkkyys, sävykäyrä, rae, sumeus,
pakkaus, naarmut). Tavoite: alkuperäisen kuvan a/b. Häviöt kuten DDColorin virallisessa
koulutuksessa: L1(ab), havaintohäviö (VGG16-BN), GAN ja värikkyys, sekä kromahäviö: ennustetun ja
oikean värikkyyden ero 16x16-ruuduittain. Ilman sitä L1 vetää mallia harmaaseen (varma arvaus),
ja DDColorin oma koulutus (tavoitteen värien vahvistus 1.2) taas tekee siitä ylivärikkään.
Kooderi (ConvNeXt-L) jäädytetään,
dekooderia koulutetaan – vähemmän muistia ja vähemmän ylisovitusta pienellä aineistolla.

Käyttö: python jatkokouluta.py --aineisto KANSIO [KANSIO ...] --painot pytorch_model.bin --ulos KANSIO
"""
import argparse, io, json, math, random, sys, time, types
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from PIL import Image, ImageFilter
from skimage import color as skcolor

REPO = Path(__file__).resolve().parents[1] / "DDColor"


def tuo_ddcolor():
    """DDColorin arkkitehtuuri ja häviöt ilman basicsr:n __init__-tiedostoja (ne vaativat OpenCV:n)."""
    for nimi, p in [("basicsr", REPO / "basicsr"), ("basicsr.archs", REPO / "basicsr/archs"),
                    ("basicsr.utils", REPO / "basicsr/utils"), ("basicsr.losses", REPO / "basicsr/losses"),
                    ("ddcolor", REPO / "ddcolor")]:
        m = types.ModuleType(nimi)
        m.__path__ = [str(p)]
        sys.modules[nimi] = m
    sys.path.insert(0, str(REPO))
    import importlib
    malli = importlib.import_module("ddcolor.model")
    diskr = importlib.import_module("basicsr.archs.discriminator_arch")
    haviot = importlib.import_module("basicsr.losses.losses")
    malli.MultiScaleColorDecoder.forward = _varidekooderi_tyylilla
    return malli.DDColor, diskr.DynamicUNetDiscriminator, haviot


# ---------------------------------------------------------------- filmityyli
# 0 automaattinen (ei ehtoa), 1 technicolor, 2 eastmancolor, 3 kodachrome, 4 moderni (tyylit.py)
TYYLEJA = 5


def _varidekooderi_tyylilla(self, x, img_features):
    """MultiScaleColorDecoder.forward, johon lisätty filmityyli: tyylivektori (self._tyyli [N,C]) lisätään
    kaikkiin värikyselyihin. Tyyli 0 (automaattinen) opitaan kaikkien tyylien sekoituksena."""
    src, pos = self._get_src_and_pos(x)
    bs = src[0].shape[1]
    query_embed = self.query_embed.weight.unsqueeze(1).repeat(1, bs, 1)
    output = self.query_feat.weight.unsqueeze(1).repeat(1, bs, 1)
    tyyli = getattr(self, "_tyyli", None)
    if tyyli is not None:
        output = output + tyyli.unsqueeze(0)
    for i in range(self.num_layers):
        level_index = i % self.num_feature_levels
        output = self.transformer_cross_attention_layers[i](output, src[level_index], memory_mask=None,
                                                             memory_key_padding_mask=None, pos=pos[level_index], query_pos=query_embed)
        output = self.transformer_self_attention_layers[i](output, tgt_mask=None, tgt_key_padding_mask=None, query_pos=query_embed)
        output = self.transformer_ffn_layers[i](output)
    decoder_output = self.decoder_norm(output).transpose(0, 1)
    color_embed = self.color_embed(decoder_output)
    return torch.einsum("bqc,bchw->bqhw", color_embed, img_features)


def lisaa_tyyli(g):
    """Tyyliupotus DDColor-malliin (nollina alussa: vanhat painot toimivat sellaisinaan)."""
    g.tyyli_upotus = torch.nn.Linear(TYYLEJA, g.decoder.color_decoder.hidden_dim, bias=False)
    torch.nn.init.zeros_(g.tyyli_upotus.weight)
    return g


def lataa_painot(g, polku):
    sd = torch.load(polku, map_location="cpu", weights_only=True)
    sd = sd.get("params", sd)
    puuttuvat, ylim = g.load_state_dict(sd, strict=False)
    assert not ylim and set(puuttuvat) <= {"tyyli_upotus.weight"}, (puuttuvat, ylim)
    return puuttuvat


def aja(g, x, tyyli):
    """tyyli: one-hot [N, TYYLEJA] (tai None = automaattinen)."""
    g.decoder.color_decoder._tyyli = None if tyyli is None else g.tyyli_upotus(tyyli)
    return g(x)


# ---------------------------------------------------------------- väriavaruus (torch, kuten DDColor)
def lab2rgb(lab):
    L, a, b = lab[:, 0:1], lab[:, 1:2], lab[:, 2:3]
    y = (L + 16.0) / 116.0
    x = a / 500.0 + y
    z = (y - b / 200.0).clamp(min=0)
    xyz = torch.cat([x, y, z], 1)
    xyz = torch.where(xyz > 0.2068966, xyz ** 3, (xyz - 16.0 / 116.0) / 7.787)
    xyz = xyz * torch.tensor([0.95047, 1.0, 1.08883], device=lab.device).view(1, 3, 1, 1)
    m = torch.tensor([[3.240481340, -1.53715152, -0.49853633], [-0.96925495, 1.875990000, 0.041555930],
                      [0.055646640, -0.20404134, 1.057311070]], device=lab.device)
    rgb = torch.einsum("ij,bjhw->bihw", m, xyz)
    rgb = torch.where(rgb > 0.0031308, 1.055 * rgb.clamp(min=1e-8) ** (1 / 2.4) - 0.055, 12.92 * rgb)
    return rgb.clamp(0, 1)


# ---------------------------------------------------------------- mustavalkofilmin simulointi
def srgb_lin(x):
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def lin_srgb(x):
    return np.where(x <= 0.0031308, 12.92 * x, 1.055 * np.clip(x, 0, None) ** (1 / 2.4) - 0.055)


def mustavalkofilmi(rgb, r):
    """rgb 0..1 (H,W,3) -> harmaa 0..1 (H,W). Satunnainen filmin ja siirron simulointi."""
    lin = srgb_lin(rgb)
    if r.random() < 0.15:  # ortokromaattinen: punainen lähes musta
        w = np.array([r.uniform(0.02, 0.1), r.uniform(0.55, 0.75), r.uniform(0.25, 0.4)])
    else:  # pankromaattinen
        w = np.array([r.uniform(0.18, 0.38), r.uniform(0.45, 0.65), r.uniform(0.06, 0.22)])
    g = lin @ (w / w.sum())
    g = lin_srgb(g)
    # sävykäyrä: gamma, kontrasti, mustan ja valkoisen taso
    g = np.clip(g, 0, 1) ** r.uniform(0.8, 1.25)
    k = r.uniform(0.0, 0.35)
    g = g + k * (g - 0.5) * (1 - np.abs(2 * g - 1)) if r.random() < 0.5 else g
    musta, valko = r.uniform(0.0, 0.08), r.uniform(0.88, 1.0)
    g = musta + (valko - musta) * np.clip(g, 0, 1)
    im = Image.fromarray((np.clip(g, 0, 1) * 255).astype(np.uint8))
    if r.random() < 0.6:
        im = im.filter(ImageFilter.GaussianBlur(r.uniform(0.3, 1.3)))
    g = np.asarray(im, np.float32) / 255.0
    # filmin rae (hieman sumennettu kohina)
    if r.random() < 0.8:
        rae = np.random.default_rng(r.randrange(1 << 30)).normal(0, r.uniform(0.005, 0.035), g.shape).astype(np.float32)
        rae = np.asarray(Image.fromarray(((rae + 0.5) * 255).clip(0, 255).astype(np.uint8)).filter(
            ImageFilter.GaussianBlur(r.uniform(0.3, 0.8))), np.float32) / 255.0 - 0.5
        g = g + rae
    # naarmut ja pölyt
    if r.random() < 0.15:
        h, w_ = g.shape
        for _ in range(r.randint(1, 4)):
            x = r.randrange(w_)
            g[:, max(0, x - 1):x + 1] += r.choice([-1, 1]) * r.uniform(0.05, 0.25)
        for _ in range(r.randint(0, 20)):
            y, x = r.randrange(h), r.randrange(w_)
            g[max(0, y - 2):y + 2, max(0, x - 2):x + 2] = r.choice([0.02, 0.98])
    g = np.clip(g, 0, 1)
    # videopakkaus
    if r.random() < 0.5:
        b = io.BytesIO()
        Image.fromarray((g * 255).astype(np.uint8)).save(b, "JPEG", quality=r.randint(45, 92))
        g = np.asarray(Image.open(b), np.float32) / 255.0
    return g


def pehmea_kohina(h, w, r, karkeus=8):
    """Pehmeä satunnainen kenttä 0..1 (muutama mittakaava päällekkäin), varjojen ja savun muotoihin."""
    g = np.random.default_rng(r.randrange(1 << 30))
    k = np.zeros((h, w), np.float32)
    for oktaavi, paino in ((karkeus, 1.0), (karkeus * 2, 0.5), (karkeus * 4, 0.25)):
        pieni = g.random((max(2, h * oktaavi // max(h, w)), max(2, w * oktaavi // max(h, w)))).astype(np.float32)
        k += paino * np.asarray(Image.fromarray((pieni * 255).astype(np.uint8)).resize((w, h), Image.BICUBIC), np.float32) / 255.0
    k = (k - k.min()) / max(k.max() - k.min(), 1e-6)
    return k


def varjo(rgb, r):
    """Pehmeä varjo: valoa vähennetään lineaarisessa väriavaruudessa. Sävy säilyy, joten tavoitteessa
    varjon alla oleva iho tai esine on yhä värillinen – malli oppii värittämään myös varjoissa."""
    h, w, _ = rgb.shape
    m = pehmea_kohina(h, w, r, karkeus=r.choice([3, 4, 6]))
    m = np.clip((m - r.uniform(0.35, 0.65)) * r.uniform(4, 12), 0, 1)[..., None]
    tummuus = r.uniform(0.12, 0.45)
    lin = srgb_lin(rgb) * (1 - m * (1 - tummuus))
    return np.clip(lin_srgb(lin), 0, 1).astype(np.float32)


def savu(rgb, r):
    """Läpikuultava, epätasainen savu tai usva. Tavoite lasketaan savuisesta kuvasta: savu itse on harmaa,
    mutta ohuen savun takana olevat kohteet ovat yhä osittain värillisiä."""
    h, w, _ = rgb.shape
    m = pehmea_kohina(h, w, r, karkeus=r.choice([4, 6, 10]))
    m = np.clip((m - r.uniform(0.2, 0.5)) * r.uniform(1.5, 4), 0, 1)[..., None] * r.uniform(0.35, 0.8)
    harmaa = r.uniform(0.55, 0.92)
    savuvari = np.array([harmaa * r.uniform(0.97, 1.03), harmaa, harmaa * r.uniform(0.95, 1.05)], np.float32)
    return np.clip((1 - m) * rgb + m * savuvari, 0, 1).astype(np.float32)


def lahde_ja_elokuva(f):
    lahde = f.parent.parent.name
    return lahde, f"{lahde}_" + f.stem[len(lahde) + 1:].rsplit("_", 1)[0]


class Aineisto(torch.utils.data.Dataset):
    """Palauttaa (L, ab, tyyli). Elokuvakuvista korjataan haalistuma (haalistuma.py) ennen kaikkea muuta,
    ja tyyli tulee tyylit.json:sta; koulutuksessa osa kuvista ilman tyyliä (automaattinen)."""
    def __init__(self, tiedostot, koko, kouluta=True, haalistuma=None, tyylit=None, ilman_tyylia=0.25):
        self.t, self.koko, self.kouluta = tiedostot, koko, kouluta
        self.haalistuma, self.ilman = haalistuma or {}, ilman_tyylia
        self.tyyli = []
        for f in tiedostot:
            lahde, el = lahde_ja_elokuva(f)
            s = 0
            if tyylit:
                s = tyylit["elokuvat"].get(el, tyylit["oletus"].get(lahde, 0))
            self.tyyli.append(s)

    def __len__(self):
        return len(self.t)

    def __getitem__(self, i):
        r = random.Random() if self.kouluta else random.Random(i)
        im = Image.open(self.t[i]).convert("RGB")
        w, h = im.size
        # satunnainen rajaus ja skaalaus; joka kolmas koko kuva (kaukaiset ihmiset ja kasvot pieninä)
        if not self.kouluta or r.random() < 0.33:
            s = min(w, h)
        else:
            s = int(min(w, h) * r.uniform(0.5, 1.0))
        x, y = r.randint(0, w - s), r.randint(0, h - s)
        im = im.crop((x, y, x + s, y + s)).resize((self.koko, self.koko), Image.BICUBIC)
        if self.kouluta and r.random() < 0.5:
            im = im.transpose(Image.FLIP_LEFT_RIGHT)
        rgb = np.asarray(im, np.float32) / 255.0
        h = self.haalistuma.get(lahde_ja_elokuva(self.t[i])[1])
        if h:
            a, b, gam = (np.asarray(v, np.float32) for v in h)
            rgb = np.clip(rgb * a + b, 0, 1) ** gam
        if self.kouluta and r.random() < 0.3:
            rgb = varjo(rgb, r)
        if self.kouluta and r.random() < 0.15:
            rgb = savu(rgb, r)
        ab = skcolor.rgb2lab(rgb)[..., 1:].astype(np.float32)
        g = mustavalkofilmi(rgb, r)
        L = skcolor.rgb2lab(np.repeat(g[..., None], 3, 2))[..., :1].astype(np.float32)
        s = self.tyyli[i]
        if self.kouluta and r.random() < self.ilman:
            s = 0
        return torch.from_numpy(L).permute(2, 0, 1), torch.from_numpy(ab).permute(2, 0, 1), F.one_hot(torch.tensor(s), TYYLEJA).float()


def kromaero(out_ab, ab):
    """Värikkyyden ero 16x16- ja 32x32-ruuduittain (molempiin suuntiin): estää harmaantumisen ja
    liioittelun; tiheämpi ruudukko rankaisee myös pienistä harmaiksi jääneistä alueista (kaukaiset kasvot)."""
    k_out = torch.sqrt(out_ab[:, 0:1] ** 2 + out_ab[:, 1:2] ** 2 + 1e-6)
    k_gt = torch.sqrt(ab[:, 0:1] ** 2 + ab[:, 1:2] ** 2 + 1e-6)
    return 0.5 * sum((F.adaptive_avg_pool2d(k_out, n) - F.adaptive_avg_pool2d(k_gt, n)).abs().mean() for n in (16, 32))


def tiedostot(kansiot):
    v = []
    for k in kansiot:
        v += sorted(Path(k).rglob("*.jpg"))
    return v


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--aineisto", nargs="+", required=True)
    p.add_argument("--painot", required=True)
    p.add_argument("--ulos", required=True)
    p.add_argument("--koko", type=int, default=384)
    p.add_argument("--era", type=int, default=4)
    p.add_argument("--askeleet", type=int, default=20000)
    p.add_argument("--lr", type=float, default=2e-5)
    p.add_argument("--gan", type=float, default=0.3)
    p.add_argument("--kroma", type=float, default=0.1)
    p.add_argument("--varikkyys", type=float, default=0.5)
    p.add_argument("--validointi", type=int, default=300)
    p.add_argument("--avaa-enkooderi", type=int, default=0,
                   help="ConvNeXt-enkooderin viimeiset N vaihetta (0–4) koulutetaan mukana")
    p.add_argument("--enk-lr", type=float, default=0.25, help="enkooderin oppimisnopeus suhteessa --lr:ään")
    p.add_argument("--haalistuma", help="haalistuma.py:n tulos (elokuvakohtainen värikorjaus)")
    p.add_argument("--tyylit", help="tyylit.py:n tulos (filmityyli ehtona)")
    p.add_argument("--tyyli-lr", type=float, default=5e-4)
    p.add_argument("--tuplaa", nargs="*", default=["finna"],
                   help="lähteet, joiden kuvat otetaan koulutukseen kahdesti (kansion nimi)")
    p.add_argument("--testi", action="store_true", help="vain muistikoe muutamalla askeleella")
    a = p.parse_args()
    ulos = Path(a.ulos)
    ulos.mkdir(parents=True, exist_ok=True)
    DDColor, Diskriminaattori, haviot = tuo_ddcolor()
    laite = torch.device("cuda")
    torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cudnn.allow_tf32 = True

    kaikki = tiedostot(a.aineisto)
    random.Random(0).shuffle(kaikki)
    val, koul = kaikki[:a.validointi], kaikki[a.validointi:]
    # suomalaiset kuvat (Finna) kahdesti: muuten amerikkalainen elokuva-aineisto hallitsisi;
    # samoin pienet "modernit" lähteet (Kodachrome, Commons), kun niitä halutaan painottaa
    koul = koul + [f for f in koul if f.parent.parent.name in a.tuplaa]
    haal = json.loads(Path(a.haalistuma).read_text(encoding="utf-8")) if a.haalistuma else None
    tyylit = json.loads(Path(a.tyylit).read_text(encoding="utf-8")) if a.tyylit else None
    (ulos / "validointi.txt").write_text("\n".join(map(str, val)), encoding="utf-8")
    print(f"aineisto: {len(koul)} koulutus, {len(val)} validointi", flush=True)

    g = DDColor(encoder_name="convnext-l", decoder_name="MultiScaleColorDecoder", input_size=(512, 512),
                num_output_channels=2, last_norm="Spectral", do_normalize=False, num_queries=100, num_scales=3, dec_layers=9)
    lisaa_tyyli(g)
    print("painot ladattu, uusina:", lataa_painot(g, a.painot), flush=True)
    g.to(laite)
    if not tyylit:
        g.tyyli_upotus.weight.requires_grad = False
    for q in g.encoder.parameters():
        q.requires_grad = False
    # jäädytetyllä enkooderilla validointi tasaantui (kierros 3): avataan syvimmät vaiheet, joissa on
    # kohteiden tunnistus (iho, vaatteet), pienellä oppimisnopeudella; matalat reunapiirteet pysyvät
    arch = g.encoder.arch
    enk = []
    for i in range(4 - a.avaa_enkooderi, 4):
        for m in (arch.downsample_layers[i], arch.stages[i], getattr(arch, f"norm{i}")):
            for q in m.parameters():
                q.requires_grad = True
                enk.append(q)
    print(f"enkooderista koulutetaan {sum(q.numel() for q in enk) / 1e6:.1f} M parametria", flush=True)
    g.encoder.eval()
    d = Diskriminaattori(n_channels=3, nf=64).to(laite)
    ema = {k: v.detach().clone() for k, v in g.state_dict().items()}

    pix = haviot.L1Loss(loss_weight=0.1).to(laite)
    perc = haviot.PerceptualLoss(layer_weights={"conv1_1": 0.0625, "conv2_1": 0.125, "conv3_1": 0.25, "conv4_1": 0.5, "conv5_1": 1.0},
                                 vgg_type="vgg16_bn", use_input_norm=True, range_norm=False, perceptual_weight=5.0,
                                 style_weight=0, criterion="l1").to(laite)
    gan = haviot.GANLoss("vanilla", real_label_val=1.0, fake_label_val=0.0, loss_weight=a.gan).to(laite)
    vari = haviot.ColorfulnessLoss(loss_weight=a.varikkyys).to(laite)

    erilliset = {id(q) for q in enk} | {id(g.tyyli_upotus.weight)}
    opt_g = torch.optim.AdamW([{"params": [q for q in g.parameters() if q.requires_grad and id(q) not in erilliset]},
                               *([{"params": enk, "lr": a.lr * a.enk_lr}] if enk else []),
                               *([{"params": [g.tyyli_upotus.weight], "lr": a.tyyli_lr, "weight_decay": 0.0}] if tyylit else [])],
                              lr=a.lr, weight_decay=0.01, betas=(0.9, 0.99))
    opt_d = torch.optim.Adam(d.parameters(), lr=1e-4, betas=(0.9, 0.99))
    lataaja = torch.utils.data.DataLoader(Aineisto(koul, a.koko, True, haal, tyylit), batch_size=a.era, shuffle=True,
                                          num_workers=6, drop_last=True, persistent_workers=True)
    val_lataaja = torch.utils.data.DataLoader(Aineisto(val, a.koko, False, haal, tyylit), batch_size=a.era, num_workers=2)

    def validoi():
        """(ab-L1, värikkyyssuhde ennuste/oikea)"""
        g.eval()
        s, n, k_out, k_gt = 0.0, 0, 0.0, 0.0
        with torch.no_grad(), torch.autocast("cuda", dtype=torch.bfloat16):
            for L, ab, t in val_lataaja:
                L, ab = L.to(laite), ab.to(laite)
                x = lab2rgb(torch.cat([L, torch.zeros_like(ab)], 1))
                o = aja(g, x, t.to(laite)).float()
                s += (o - ab).abs().mean().item() * len(L)
                k_out += torch.hypot(o[:, 0], o[:, 1]).mean().item() * len(L)
                k_gt += torch.hypot(ab[:, 0], ab[:, 1]).mean().item() * len(L)
                n += len(L)
        g.train()
        g.encoder.eval()
        return s / max(n, 1), k_out / max(k_gt, 1e-6)

    def pisteet(v):
        # pienempi parempi: virhe, kerrottuna värikkyyden poikkeamalla todellisesta
        return v[0] * (1.0 + 2.0 * abs(1.0 - v[1]))

    loki = open(ulos / "loki.jsonl", "a", encoding="utf-8")
    alku = validoi()
    print(f"validointi ennen: ab-L1 {alku[0]:.3f}, värikkyys {alku[1]:.2f}", flush=True)
    loki.write(json.dumps({"askel": 0, "val_ab_l1": alku[0], "varikkyys": alku[1]}) + "\n")
    g.train()
    g.encoder.eval()
    askel, t0 = 0, time.time()
    paras = pisteet(alku)
    while askel < a.askeleet:
        for L, ab, t in lataaja:
            L, ab, t = L.to(laite, non_blocking=True), ab.to(laite, non_blocking=True), t.to(laite, non_blocking=True)
            x = lab2rgb(torch.cat([L, torch.zeros_like(ab)], 1))
            with torch.no_grad():
                gt = lab2rgb(torch.cat([L, ab], 1))
            # generaattori
            for q in d.parameters():
                q.requires_grad = False
            with torch.autocast("cuda", dtype=torch.bfloat16):
                out_ab = aja(g, x, t).float()
            out = lab2rgb(torch.cat([L, out_ab], 1))
            with torch.autocast("cuda", dtype=torch.bfloat16):
                l_pix = pix(out_ab, ab)
                l_perc, _ = perc(out, gt)
                l_gan = gan(d(out), True, is_disc=False)
            l_col = vari(out)
            l_kr = a.kroma * kromaero(out_ab, ab)
            l_g = l_pix + l_perc + l_gan + l_col + l_kr
            opt_g.zero_grad(set_to_none=True)
            l_g.backward()
            torch.nn.utils.clip_grad_norm_(g.parameters(), 1.0)
            opt_g.step()
            # diskriminaattori
            for q in d.parameters():
                q.requires_grad = True
            with torch.autocast("cuda", dtype=torch.bfloat16):
                l_d = gan(d(gt), True, is_disc=True) + gan(d(out.detach()), False, is_disc=True)
            opt_d.zero_grad(set_to_none=True)
            l_d.backward()
            opt_d.step()
            with torch.no_grad():
                for k, v in g.state_dict().items():
                    if v.dtype.is_floating_point:
                        ema[k].mul_(0.999).add_(v, alpha=0.001)
            askel += 1
            if askel % 50 == 0 or a.testi:
                mt = torch.cuda.max_memory_allocated() / 2**30
                print(f"askel {askel}: pix {l_pix.item():.3f} perc {l_perc.item():.3f} gan {l_gan.item():.3f} "
                      f"col {l_col.item():.3f} kroma {l_kr.item():.3f} d {l_d.item():.3f}  {(time.time() - t0) / askel:.2f} s/askel, muisti {mt:.1f} Gt", flush=True)
            if a.testi and askel >= 5:
                return
            if askel % 1000 == 0 or askel == a.askeleet:
                talteen = {k: v.clone() for k, v in g.state_dict().items()}
                g.load_state_dict(ema)
                v = validoi()
                print(f"askel {askel}: validointi ab-L1 {v[0]:.3f}, värikkyys {v[1]:.2f} "
                      f"(alussa {alku[0]:.3f} / {alku[1]:.2f})", flush=True)
                loki.write(json.dumps({"askel": askel, "val_ab_l1": v[0], "varikkyys": v[1]}) + "\n")
                loki.flush()
                torch.save({"params": {k: (t.half() if t.is_floating_point() else t) for k, t in ema.items()}},
                           ulos / f"askel_{askel}.pt")
                if pisteet(v) < paras:
                    paras = pisteet(v)
                    torch.save({"params": ema}, ulos / "paras.pt")
                g.load_state_dict(talteen)
            if askel >= a.askeleet:
                break


if __name__ == "__main__":
    main()
