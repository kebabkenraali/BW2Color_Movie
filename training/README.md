# Training tools

Python tools that build the training data and train the two models shipped with BW2Color Movie:
the fine-tuned colorization model `ddcolor_suomi` and the uncoloured-area detector. The script names and
comments are in Finnish; this page explains the pipeline in English.

## Setup

- Python 3.12+ with PyTorch (CUDA), NumPy, Pillow, scikit-image, onnx, onnxruntime.
- For the film downloader (`elokuvat_lataa.py`): PyAV (`pip install av`) – it reads individual frames from
  Internet Archive videos without downloading whole films.
- The DDColor source code next to this folder: `git clone https://github.com/piddnad/DDColor` into the
  repository root (`DDColor/`), and the ModelScope DDColor weights (`pytorch_model.bin`) as the starting point.
  The scripts import the DDColor architecture directly and bypass its package `__init__` (which needs OpenCV).

## 1. Training data (freely licensed only)

| Script | Source | Licence filter |
|---|---|---|
| `finna_lataa.py` | Finna.fi photographs 1940–2000 | usage rights A + named licence (PDM / CC0) |
| `prelinger_lataa.py` | Prelinger Archives colour films | `licenseurl` = public domain |
| `elokuvat_lataa.py` | any Internet Archive query (FedFlix, feature films, Prelinger amateur films) | `--vaadi-pd` requires a public-domain licence URL |
| `commons_lataa.py` | Wikimedia Commons Quality images 2001– | CC0, camera EXIF |
| `kuvasuodatin.py` | – | drops faded frames, title cards, empty frames |
| `piirretyt.py`, `poista_piirretyt.py` | – | drops animated films (DINOv2 kNN, decided per film) |
| `elokuvat_suodata.py` | – | drops foreign productions (URAA) and wrongly marked titles from the feature films |
| `palauta_lahteet.py` | – | rebuilds a source list from file names via Internet Archive metadata |
| `lahdeluettelo.py` | – | writes the credits list `TRAINING_DATA.txt` |

## 2. Fade correction and film styles

- `haalistuma.py` computes a per-film correction for faded colour film (per-channel level stretch plus a
  partial mid-tone balance, computed over all frames of the same film so that a sunset or a forest keeps its
  own colour). The result `haalistuma.json` is applied on the fly during training.
- `tyylit.py` assigns each film a style from its Internet Archive description: 1 Technicolor, 2 Eastmancolor,
  3 Kodachrome, 4 modern (Commons), 0 automatic (Finna, and 25 % of all samples during training).

## 3. Fine-tuning `ddcolor_suomi`

`jatkokouluta.py` fine-tunes DDColor on simulated black-and-white film made from the colour images
(panchromatic/orthochromatic response, tone curve, grain, blur, scratches, video compression) with shadow,
smoke and distance augmentations. Losses: L1 (ab), VGG perceptual, GAN, colourfulness and a chroma loss on a
16×16 and 32×32 grid. The style is a learned vector added to all colour queries of the decoder.

```
python jatkokouluta.py --aineisto DATA/finna/kuvat DATA/prelinger/kuvat ... \
    --painot RUN_PREV/askel_6000.pt --ulos RUN --era 3 --koko 384 --askeleet 6000 --lr 1e-5 --kroma 0.2 \
    --avaa-enkooderi 1 --haalistuma DATA/haalistuma.json --tyylit DATA/tyylit.json \
    --tuplaa finna finna_1950_1980 kodachrome commons
python vertaa.py --painot OLD.pt --uusi RUN/askel_N.pt --ffmpeg ffmpeg.exe --video "film.mp4:300,1200" --tyylit 0 1 2 3 4 --ulos sheet.jpg
python vie_onnx.py RUN/askel_N.pt ddcolor_suomi.onnx 1.3     # ONNX with inputs "input" and "tyyli", output gain
```

The fine-tuned model desaturates slightly (colourfulness ratio ~0.8 in validation); the export multiplies
a/b by a gain (`varikkyys.py` measures the ratio).

## 4. Uncoloured-area detector

`tunnistin_aineisto.py` creates training pairs: object-sized regions (SLIC, not touching the border) and skin
are desaturated in DDColor colorizations; large grey backgrounds are desaturated without a label so that grey
walls and skies are not flagged. `tunnistin_kouluta.py` trains a small U-Net (3.2 M parameters),
`tunnistin_testi.py` / `tunnistin_etsi.py` test it on real films and `vie_tunnistin.py` exports it to ONNX.
