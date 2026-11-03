# BW2Color Movie

Colorize black-and-white feature films on Windows – with a model fine-tuned on 1950s–80s colour film,
selectable film-stock styles, automatic quality control and a keyframe ("exemplar") workflow for full control.

BW2Color Movie is a native Windows application (Rust, egui, ONNX Runtime with DirectML). It runs on any
DirectX 12 graphics card (NVIDIA, AMD, Intel) and falls back to the CPU. Everything runs locally.

## Features

- **Colorize** a whole film in one go. The recommended pipeline colorizes, checks every frame and repairs
  what went wrong: grey frames and uncoloured patches are refilled, sudden colour jumps are smoothed.
- **Own fine-tuned model `ddcolor_suomi`** (DDColor fine-tuned on freely licensed 1950s–80s colour material,
  see [MODEL_CARD.md](MODEL_CARD.md)) with **film styles**:
  | Model name | Look |
  |---|---|
  | `ddcolor_suomi` | automatic – a mix of all styles |
  | `ddcolor_suomi_technicolor` | saturated 1950s feature film |
  | `ddcolor_suomi_eastmancolor` | 1960s–70s film stock |
  | `ddcolor_suomi_kodachrome` | bright, crisp amateur film |
  | `ddcolor_suomi_moderni` | modern, 21st-century colours |
- **Faded film was corrected in the training data**, so the model learns the colours as they were shot,
  not the magenta cast of 50-year-old prints.
- **Uncoloured-area detector** – a small own model that finds faces, people or objects left grey in an
  otherwise coloured frame; they are refilled during quality control.
- **Colour memory** – colours follow motion (optical flow) through a scene, so a coat does not change colour
  from frame to frame. **Scene memory** (DINOv2, optional add-on) keeps an object's colour across cuts.
- **Duplicate-frame detection** – e.g. a 25 fps film converted to 30 fps repeats every 6th frame; copies are
  dropped and the result is written at the true frame rate (duration and sound unchanged).
- **Repair** an already colorized video: flicker removal, colour lock for skin/sky/vegetation, refilling grey
  areas, brightness and sharpness from the black-and-white original.
- **Exemplar** workflow: mark keyframes, pick the best of six suggestions (automatic, + reference,
  Technicolor style, Eastmancolor style, palette transfer, manual HSV) and the program interpolates smoothly between them.
- **Assistant** (optional, local vision model via [Ollama](https://ollama.com)): recommends the most natural
  suggestion, analyses scenes automatically and writes an HTML quality report.
- **Random preview**: run the current settings on a random 5-second clip and watch it side by side with the
  source.

## Installation

1. Download `BW2Color-Movie-<version>-win64.zip` from the
   [Releases](https://github.com/kebabkenraali/BW2Color_Movie/releases) page and extract it.
2. Either run `BW2Color.exe` directly from the extracted folder, or double-click `Install.cmd`, which copies the
   program to `%LOCALAPPDATA%\Programs\BW2Color Movie` and creates Start-menu and desktop shortcuts
   (`Uninstall.cmd` in the installed folder removes it again).
3. On first start the **Add-ons** tab opens: *Install FFmpeg* downloads FFmpeg (~190 MB) into the application
   folder. The colorization model `ddcolor_suomi` (all film styles) and the detector are included in the release.

Requirements: Windows 10/11 64-bit, a DirectX 12 graphics card with at least 4 GB of memory is recommended
(CPU works but is slow). The Visual C++ runtime is included. Ollama is the only optional separate program.

## Quick start

1. **1. Colorize** tab → choose the black-and-white video.
2. Method: `ddcolor_suomi` (or one of the styles). Keep *Check and repair automatically* on.
3. Press **🎲 Random preview** to try the settings on a short clip, then **▶ Start colorizing**.

The result is written next to the source as `<name>_colorized.mkv`. If the colours look unnatural, check that
Hue is 0 and Saturation 1.0 (*Reset H/S/V*).

## Tabs

| Tab | What it does |
|---|---|
| 1. Colorize | black-and-white video → colour video with one model; optional reference image and HSV adjustment |
| 2. Repair | flicker removal and grey-area refilling of an already colorized video |
| 3. Exemplar | keyframes, suggestions A–F, smooth interpolation, scene analysis |
| Stabilization & output | temporal smoothing, colour memory, scene memory, colour lock, codec, CRF, sample range |
| Add-ons | FFmpeg, DINOv2, vision model: status, download (checksums verified), remove |
| Assistant | vision model settings, scene analysis, quality report |

## Command line

```
BW2Color.exe colorize VIDEO --model ddcolor_suomi_eastmancolor --out RESULT.mkv   (colorize -> check -> repair)
BW2Color.exe check RESULT.mkv                                                     (grey frames, patches, colour jumps)
BW2Color.exe color VIDEO --model ddcolor_suomi --out RESULT.mkv                   (colorize only)
BW2Color.exe repair VIDEO --original BW.mp4
BW2Color.exe exemplar PROJECT.bw2color.json
BW2Color.exe quality RESULT.mkv --samples 40
BW2Color.exe models
BW2Color.exe install ffmpeg dinov2_small vision:qwen2.5vl:7b
BW2Color.exe help
```

Common options: `--device auto|gpu|cpu`, `--memory 0.98`, `--no-scene-memory`, `--keep-duplicates`,
`--start S --duration S`, `--codec x264|x265|nvenc --crf 14`, `--audio FILE`. See `BW2Color.exe help`.

## Building from source

```
cargo build --release
python tools/package.py        # builds dist\BW2Color Movie\ and the release zip
```

Requires Rust (stable, edition 2024) and the MSVC toolchain. ONNX Runtime and DirectML are fetched by the
`ort` crate. The self-test (`cargo build --release --features itsetesti`) drives the GUI through all main
functions; it uses local test videos and is meant for development.

The training tools for `ddcolor_suomi` and the detector are in [`training/`](training/) (Python, PyTorch).
The program's internal identifiers and comments are in Finnish; the user interface is in English.

## Licences

- BW2Color Movie's own code: MIT – see [LICENSE](LICENSE).
- `ddcolor_suomi` model: Apache-2.0 (derivative of DDColor) – see [MODEL_CARD.md](MODEL_CARD.md); training data
  sources and credits in [TRAINING_DATA.txt](TRAINING_DATA.txt).
- All components and their licences: [LICENSES.txt](LICENSES.txt). FFmpeg (GPL-3.0) is downloaded separately
  by the user and run as a separate program; it is not part of the release.

## Acknowledgements

DDColor (Kang et al., piddnad/DDColor), DINOv2 (Meta AI), ONNX Runtime, egui,
FFmpeg, the archives that publish their collections openly – Finna.fi and the Finnish museums,
Prelinger Archives, Internet Archive, Wikimedia Commons and the US National Archives (FedFlix).

Built with [Claude Code](https://claude.com/claude-code) by Anthropic.
