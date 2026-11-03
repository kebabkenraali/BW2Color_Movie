"""Pudottaa elokuva-aineistosta (IA feature_films) elokuvat, joiden public domain -asema Yhdysvalloissakin
on epävarma, ja siirtää niiden kuvat sivuun (aineisto/elokuvat_pois/):
- ulkomaiset tuotannot: URAA (1996) palautti tekijänoikeuden ulkomaisille teoksille, jotka olivat
  Yhdysvalloissa vapaita vain muotoseikkojen (puuttuva merkintä, uusimatta jättäminen) takia;
- tunnetusti suojatut, väärin PD-merkityt (esim. Disney).
Lista on tehty otsikoista käsin, varovaisuuden puolelle: epävarmat pudotetaan.

Käyttö: python elokuvat_suodata.py AINEISTOKANSIO
"""
import json, re, shutil, sys
from pathlib import Path

POIS = [
    # suojatut / väärin merkityt
    r"20,000 Leagues",
    # Iso-Britannia, Australia, Kanada, Etelä-Afrikka, Irlanti
    r"Genevieve", r"Purple Plain", r"Diamond.? Wizard", r"Long John Silver", r"Happy Go Lovely", r"Black Tent",
    r"Campbell's Kingdom", r"Windom's Way", r"Harry Black", r"Inn of the Sixth", r"Sapphire", r"North West Frontier",
    r"Millionairess", r"Sword of Lancelot", r"Lancelot and Guinevere", r"Woman of Straw", r"High Wind in Jamaica",
    r"She Beast", r"Long Duel", r"Cape Town Affair", r"Twinky", r"Horror Express", r"Eagle in a Cage",
    r"Satanic Rites", r"^Gold \(1974", r"Funeral for an Assassin", r"Patrick \(1978", r"Dominique", r"Absolution",
    r"The Agency", r"Cry of the Innocent", r"Cries in the Night", r"Born for Hell", r"Captain Scarlett",
    # Ranska, Saksa, DDR, Italia, Espanja, Kreikka, Egypti
    r"Napoleon", r"Liane", r"Giant of Marathon", r"Hercules", r"Ercole", r"Devils? Cav", r"cavalieri", r"Maratona",
    r"Assignment Outer Space", r"Collants", r"Black Tights", r"Colossus and the Amazon", r"Amazzoni",
    r"Spaceship on Venus", r"schweigende Stern", r"David and Goliath", r"Esther and the King", r"Space Men",
    r"Maciste", r"Tessaglia", r"Orazi", r"prigioniere", r"tiranno", r"gladiat", r"Giulio Cesare", r"Duello nel Texas",
    r"Son of Hercules", r"Sons of Hercules", r"eroe di Babilonia", r"Ursus", r"Golia", r"Simbad", r"Bloody Pit",
    r"boia scarlatto", r"giorni dell'odio", r"Da uomo a uomo", r"Qui[eé]n sabe", r"Comanche blanco", r"Commandos",
    r"cercati un posto", r"Testa di sbarco", r"Venere selvaggia", r"Counting the Years", r"El Alamein",
    r"collina degli stivali", r"Assig?nment.?Terror", r"monstruos del terror", r"Sam Walbash", r"muerte viviente",
    r"Arde baby", r"pelle dura", r"Blutiger Freitag", r"mala ordina", r"uomo da rispettare", r"Shangai Joe",
    r"campana del infierno", r"mansi[oó]n de la locura", r"Witches Mountain", r"Medusa", r"Chi sei", r"castello delle donne",
    r"Profondo rosso", r"Death Rage", r"rabbia agli occhi", r"Squadra antiscippo", r"War of the Planets",
    r"Amore, piombo", r"dio cannibale", r"Monstroid", r"Voodoo Mission",
    # Japani, Hongkong, Kiina, Taiwan
    r"Destroy All Planets", r"Gamera", r"Satsujin ken", r"Onna hissatsu", r"Virus", r"Ninja bugeicho",
    r"Red Detachment", r"Yin and the Yang", r"Shuang xia", r"Lei tai", r"Hei ming dan", r"Meng jiao", r"Qi sha jie",
    r"Bu bu zhui zong", r"Ren hu lian", r"Yong zheng", r"Huang fei hong", r"Da wu shi", r"Real Bruce Lee",
    r"Ren gui she hu", r"Meng nan da zei", r"Tai ji ba jiao", r"Bo za", r"San mao", r"Secret Kung Fu",
]


def main():
    k = Path(sys.argv[1])
    lahde = k / "elokuvat" / "lahteet_elokuvat.jsonl"
    rivit = [json.loads(r) for r in lahde.read_text(encoding="utf-8").splitlines() if r.strip()]
    malli = re.compile("|".join(POIS), re.I)
    pois = [r for r in rivit if malli.search(str(r.get("otsikko") or ""))]
    jaa = [r for r in rivit if r not in pois]
    sivuun = k / "elokuvat_pois" / "kuvat"
    sivuun.mkdir(parents=True, exist_ok=True)
    n = 0
    for r in pois:
        for f in (k / "elokuvat" / "kuvat").glob(f"elokuvat_{glob_turva(r['id'][:60])}_*.jpg"):
            shutil.move(str(f), sivuun / f.name)
            n += 1
    lahde.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in jaa), encoding="utf-8")
    (k / "elokuvat_pois" / "lahteet_elokuvat_pois.jsonl").open("a", encoding="utf-8").write(
        "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in pois))
    print(f"pudotettu {len(pois)} elokuvaa ({n} kuvaa), jäljellä {len(jaa)}")


def glob_turva(s):
    return re.sub(r"([\[\]\*\?])", r"[\1]", s)


if __name__ == "__main__":
    main()
