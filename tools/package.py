# SPDX-License-Identifier: MIT
# Kokoaa julkaisupaketin: dist\BW2Color Movie\ + dist\BW2Color-Movie-<versio>-win64.zip
# (BW2Color.exe, DirectML.dll, Visual C++ -ajonaikaiskirjasto, omat mallit, ohjeet, lisenssit, asennusskriptit).
# FFmpeg ja muut mallit eivät kuulu pakettiin – ne asennetaan sovelluksen Add-ons-välilehdeltä.
# Käyttö (repon juuressa, cargo build --release ensin):
#   python tools/package.py [--ddcolor-suomi POLKU.onnx]
# Ilman --ddcolor-suomi käytetään kehityskansion mallia (..\Varita\models\ddcolor_suomi\ddcolor_suomi.onnx).
import argparse
import glob
import re
import shutil
import subprocess
import zipfile
from pathlib import Path

juuri = Path(__file__).resolve().parent.parent
p = argparse.ArgumentParser()
p.add_argument("--ddcolor-suomi", default=str(juuri.parent / "Varita" / "models" / "ddcolor_suomi" / "ddcolor_suomi.onnx"))
a = p.parse_args()

versio = re.search(r'^version = "(.+)"', (juuri / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
subprocess.run(["python", "-I", str(juuri / "tools" / "licenses.py")], check=True)

# uusin Visual Studion x64-ajonaikaiskirjasto (jaettavissa "Distributable Code" -ehdoin)
crt = sorted(glob.glob(r"C:\Program Files*\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT"),
             key=lambda p: [int(x) for x in Path(p).parent.parent.name.split(".") if x.isdigit()])
if not crt:
    raise SystemExit("Visual C++ runtime (VC\\Redist) not found")
crt = Path(crt[-1])

NIMI = "BW2Color Movie"
kohde = juuri / "dist" / NIMI
shutil.rmtree(kohde, ignore_errors=True)
kohde.mkdir(parents=True)
tiedostot = {
    "BW2Color.exe": juuri / "target" / "release" / "BW2Color.exe",
    "DirectML.dll": juuri / "target" / "release" / "DirectML.dll",
    "README.md": juuri / "README.md",
    "MODEL_CARD.md": juuri / "MODEL_CARD.md",
    "TRAINING_DATA.txt": juuri / "TRAINING_DATA.txt",
    "AUTHORS.md": juuri / "AUTHORS.md",
    "LICENSE": juuri / "LICENSE",
    "LICENSES.txt": juuri / "LICENSES.txt",
    "Install.cmd": juuri / "tools" / "Install.cmd",
    "install.ps1": juuri / "tools" / "install.ps1",
    # omat mallit
    "models/tunnistin/varittamaton.onnx": juuri / "assets" / "varittamaton.onnx",
    "models/ddcolor_suomi/ddcolor_suomi.onnx": Path(a.ddcolor_suomi),
}
for dll in ("msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
    tiedostot[dll] = crt / dll
for nimi, lahde in tiedostot.items():
    (kohde / nimi).parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(lahde, kohde / nimi)

zip_polku = juuri / "dist" / f"BW2Color-Movie-{versio}-win64.zip"
with zipfile.ZipFile(zip_polku, "w", zipfile.ZIP_DEFLATED, compresslevel=6, allowZip64=True) as z:
    for f in sorted(kohde.rglob("*")):
        if f.is_file():
            z.write(f, f"{NIMI}/{f.relative_to(kohde).as_posix()}")
print(f"{zip_polku}: {zip_polku.stat().st_size / 1048576:.1f} MB, VC runtime {crt}")
