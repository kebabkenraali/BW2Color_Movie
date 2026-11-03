"""Värittämättömien kohtien tunnistin -> ONNX. Syöte 'lab' [N,3,H,W] (L/100, a/100, b/100; H ja W
16:n monikertoja), ulos 'maski' [N,1,H,W] todennäköisyys 0..1.

Käyttö: python vie_tunnistin.py PAINOT.pt ULOS.onnx
"""
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch

sys.path.insert(0, str(Path(__file__).parent))
from tunnistin_kouluta import UNet  # noqa: E402


class Todennakoisyys(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, lab):
        return torch.sigmoid(self.m(lab))


def main():
    painot, ulos = sys.argv[1], Path(sys.argv[2])
    m = UNet()
    m.load_state_dict(torch.load(painot, map_location="cpu", weights_only=True))
    m = Todennakoisyys(m).eval()
    x = torch.rand(1, 3, 288, 512)
    with torch.inference_mode():
        y = m(x).numpy()
    ulos.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(m, (x,), str(ulos), input_names=["lab"], output_names=["maski"], opset_version=17,
                      dynamic_axes={"lab": {0: "n", 2: "h", 3: "w"}, "maski": {0: "n", 2: "h", 3: "w"}}, dynamo=False)
    s = ort.InferenceSession(str(ulos), providers=["CPUExecutionProvider"])
    for h, w in [(288, 512), (400, 704)]:
        x = np.random.rand(1, 3, h, w).astype(np.float32)
        yo = s.run(None, {"lab": x})[0]
        with torch.inference_mode():
            yt = m(torch.from_numpy(x)).numpy()
        print(f"{h}x{w}: ero max {np.abs(yo - yt).max():.5f}")
    print(f"{ulos}: {ulos.stat().st_size / 2**20:.1f} Mt")


if __name__ == "__main__":
    main()
