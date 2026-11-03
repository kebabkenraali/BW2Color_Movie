# Model card: ddcolor_suomi

`models/ddcolor_suomi/ddcolor_suomi.onnx` – the colorization model fine-tuned by the BW2Color Movie project.

## Summary

| | |
|---|---|
| Base model | DDColor (ConvNeXt-L encoder, multi-scale colour decoder), ModelScope weights – [piddnad/DDColor](https://github.com/piddnad/DDColor), Apache-2.0 |
| Task | predict the a/b colour channels of a black-and-white film frame (brightness always comes from the original) |
| Inputs | `input`: grey RGB 0..1, `[N, 3, 512, 512]`; `tyyli` (style): one-hot `[N, 5]` |
| Output | `ab`: `[N, 2, 512, 512]` (CIE Lab a/b) |
| Styles | 0 automatic, 1 Technicolor, 2 Eastmancolor, 3 Kodachrome, 4 modern |
| Licence | Apache-2.0 (derivative of DDColor) |
| Size | 934 MB (FP32 ONNX, opset 18) |

In BW2Color Movie the styles appear as separate model names: `ddcolor_suomi`, `ddcolor_suomi_technicolor`,
`ddcolor_suomi_eastmancolor`, `ddcolor_suomi_kodachrome`, `ddcolor_suomi_moderni`.

## Intended use

Colorizing black-and-white feature films and documentaries from roughly the 1930s–1980s, in particular Finnish
films, with natural, period-appropriate colours. The colours are a plausible guess, not a reconstruction of the
real colours – the model cannot know what colour a coat actually was.

## Training data

Only freely licensed material was used, because the model is distributed with the application
(full list of sources and authors in [TRAINING_DATA.txt](TRAINING_DATA.txt)):

| Source | Amount | Licence |
|---|---|---|
| Finna.fi photographs 1940–2000 (Finnish museums and archives) | 4 538 images | PDM, CC0 |
| Prelinger Archives colour films 1950–1980 | 77 films | public domain |
| Prelinger Archives amateur / Kodachrome films | 11 films | public domain |
| FedFlix – films of US federal agencies 1950–1980 | 725 films | US federal works (17 U.S.C. § 105) |
| Internet Archive feature films 1950–1980, US productions | 215 films | marked public domain (US law) |
| Wikimedia Commons Quality images 2001– | 3 999 images | CC0 |

About 46 000 training images in total (several frames per film). Filtering:

- Animated films were removed (DINOv2 classifier, decided per film) – otherwise the model learns cartoon colours.
- Frames with a strong colour cast, title cards and empty frames were removed.
- **Fade correction**: for each film a per-channel level and mid-tone correction was computed over all its
  frames and applied before training, so the model learns the colours as they were shot rather than the
  magenta or yellow cast of faded prints.
- Foreign feature films were excluded, because the URAA (1996) restored US copyright in many foreign works
  that had fallen into the US public domain through formalities, and titles known to be wrongly marked public
  domain were removed.

**Note on the feature films:** their public-domain status rests on US law (expired or unrenewed copyright,
missing notice) and on the Internet Archive's marking; in other countries some of them may still be protected.
They make up about a fifth of the training images. The model weights do not contain the images as such.

## Training

Fine-tuned from the ModelScope DDColor weights in rounds (2026-10/11):

1. Finna + Prelinger, decoder only.
2. + Finna 1940s–2000, Commons 2001–, simulated shadows, smoke and distant subjects.
3. + FedFlix and public-domain feature films (decoder only, ~45 000 images).
4. Last ConvNeXt stage unfrozen (61.6 M parameters, ¼ learning rate).
5. **Current:** fade-corrected targets, film-style conditioning (a learned vector added to all 100 colour
   queries of the decoder; 25 % of samples trained without a style), foreign feature films removed,
   Kodachrome amateur films added.

Input: simulated black-and-white film made from the colour image (random panchromatic or orthochromatic
response, tone curve, grain, blur, scratches and dust, JPEG compression), 384×384 crops.
Losses: L1 on a/b, VGG16 perceptual loss, GAN (DDColor's U-Net discriminator, weight 0.3), colourfulness loss,
and a chroma loss on 16×16 and 32×32 grids (penalises grey patches and over-saturation). EMA of the weights.
The fine-tuned model is slightly less colourful than the ground truth in validation; the ONNX export multiplies
a/b by a fixed gain.

## Evaluation

Validation set: held-out frames of the same sources (fade-corrected targets), 512×512.

| | a/b L1 (lower is better) | colourfulness vs. ground truth |
|---|---|---|
| Round 3 release (starting point of round 5) | 6.38 | 0.95 |
| Released checkpoint (round 5, step 3 000 of 6 000) | 5.90 | 0.79 (×1.26 gain in the ONNX export) |

Training continued to step 6 000, but validation error rose slightly after step 3 000, so the earlier checkpoint
was released. On real Finnish black-and-white feature films (1970s) the round-5 model removes the magenta cast
of the previous release and gives more neutral skin, wall and clothing colours. The film styles change the colours
subtly (palette and saturation), not drastically; the Kodachrome style has the least training data (11 films).

## Limitations

- Colours are plausible, not historically verified. Clothing, cars and painted walls in particular may get a
  different colour than in reality, and can change colour between shots (BW2Color Movie's colour memory and
  scene memory reduce this).
- The training data is mostly American 16 mm industrial, educational and government films and US feature
  films, plus Finnish photographs; the model is less reliable on other kinds of material.
- Very dark scenes, heavy smoke and very small distant faces may stay grey; the application's quality control
  and uncoloured-area detector refill such areas with another film style.
- Skin tones of the training material are mostly of light-skinned people.

## Uncoloured-area detector

`models/tunnistin/varittamaton.onnx` – a small U-Net (3.2 M parameters, MIT) that takes a colorized frame
(L/100, a/100, b/100) and outputs a mask of object-sized areas (faces, people, objects) left grey in an
otherwise coloured frame. Trained on DDColor colorizations of the same freely licensed material with synthetic
desaturated regions; large grey backgrounds were included as negatives. Threshold 0.65, components touching
the frame border and areas under 0.4 % of the frame are ignored.
