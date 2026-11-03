# SPDX-License-Identifier: MIT
# Luo LICENSES.txt: pohja (licenses_template.txt) + BW2Color.exe:n Rust-kirjastojen lisenssit cargo treestä.
# Käyttö (repon juuressa): python tools/licenses.py
import re
import subprocess
from pathlib import Path

juuri = Path(__file__).resolve().parent.parent
puu = subprocess.run(["cargo", "tree", "-e", "normal", "--target", "x86_64-pc-windows-msvc", "--prefix", "none",
                      "-f", "{p}|{l}"], cwd=juuri, capture_output=True, text=True, encoding="utf-8", check=True).stdout
rivit = set()
for r in puu.splitlines():
    r = re.sub(r" \((\*|proc-macro|[A-Za-z]:.*)\)", "", r).strip()
    if not r or r.startswith("varita "):
        continue
    p, _, lisenssi = r.partition("|")
    nimi, _, versio = p.partition(" ")
    rivit.add(f"  {nimi:<34} {versio:<10} {lisenssi or '(see package)'}")
teksti = (juuri / "licenses_template.txt").read_text(encoding="utf-8") + "\n".join(sorted(rivit)) + "\n"
(juuri / "LICENSES.txt").write_text(teksti, encoding="utf-8-sig", newline="\r\n")
print(f"LICENSES.txt: {len(rivit)} Rust libraries")
