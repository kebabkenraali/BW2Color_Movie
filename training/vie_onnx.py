"""Jatkokoulutettu DDColor -> ONNX samassa muodossa kuin Väritän muut DDColor-mallit:
syöte 'input' harmaa RGB 0..1 [N,3,512,512] ja 'tyyli' one-hot [N,5], ulos 'ab' [N,2,512,512]. Tarkistaa tuloksen ONNX Runtimella.

Käyttö: python vie_onnx.py PAINOT.pt ULOS.onnx [VÄRIN_VAHVISTUS]
VÄRIN_VAHVISTUS kertoo a/b:n (esim. 1.2, kun validoinnin värikkyyssuhde on noin 0.82).
"""
import sys, time
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch

sys.path.insert(0, str(Path(__file__).parent))
from jatkokouluta import TYYLEJA, aja, lisaa_tyyli, tuo_ddcolor  # noqa: E402

KOKO = 512


class Tyylilla(torch.nn.Module):
    """Kaksi syötettä: kuva ja filmityyli one-hot [N, TYYLEJA] (0 automaattinen, 1 technicolor,
    2 eastmancolor, 3 kodachrome, 4 moderni). Ulos a/b kerrottuna värin vahvistuksella."""
    def __init__(self, m, k):
        super().__init__()
        self.m, self.k = m, k

    def forward(self, x, tyyli):
        return aja(self.m, x, tyyli) * self.k


def main():
    painot, ulos = sys.argv[1], Path(sys.argv[2])
    vahvistus = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0
    DDColor, _, _ = tuo_ddcolor()
    m = DDColor(encoder_name="convnext-l", decoder_name="MultiScaleColorDecoder", input_size=[KOKO, KOKO],
                num_output_channels=2, last_norm="Spectral", do_normalize=False, num_queries=100, num_scales=3, dec_layers=9)
    sd = torch.load(painot, map_location="cpu", weights_only=True)
    sd = {k: v.float() if v.is_floating_point() else v for k, v in sd.get("params", sd).items()}
    lisaa_tyyli(m)
    print(m.load_state_dict(sd, strict=True))
    m = Tyylilla(m, vahvistus).eval()
    print(f"värin vahvistus {vahvistus}")
    x = torch.rand(2, 3, KOKO, KOKO)
    ty = torch.nn.functional.one_hot(torch.tensor([1, 3]), TYYLEJA).float()
    t = time.time()
    with torch.inference_mode():
        y = m(x, ty).numpy()
    ulos.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(m, (x, ty), str(ulos), input_names=["input", "tyyli"], output_names=["ab"], opset_version=18,
                      dynamic_axes={"input": {0: "n"}, "tyyli": {0: "n"}, "ab": {0: "n"}}, dynamo=False, external_data=False)
    s = ort.InferenceSession(str(ulos), providers=["CPUExecutionProvider"])
    yo = s.run(None, {"input": x.numpy(), "tyyli": ty.numpy()})[0]
    ero = np.abs(yo - y)
    print(f"{ulos}: {ulos.stat().st_size / 2**20:.0f} Mt, {time.time() - t:.0f} s, ero max {ero.max():.4f} ka {ero.mean():.5f}")


if __name__ == "__main__":
    main()
