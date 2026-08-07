"""Refresh tools/data/helper/items.json from hero-siege-helper.vercel.app.

The site ships a datamined item table inside one of its JavaScript bundles:
every item with its identity triple, rarity and tier — the three things the
game's own files do not hand over (names live in translationsItem.csv, but the
grade and rarity are compiled into the executable).
"""

import json
import re
import urllib.request
from pathlib import Path

SITE = "https://hero-siege-helper.vercel.app"
OUT = Path(__file__).parent / "data" / "helper" / "items.json"


def fetch(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "hs-tracker item table generator"})
    with urllib.request.urlopen(request, timeout=90) as response:
        return response.read().decode("utf-8", "replace")


page = fetch(f"{SITE}/items")
items = None
for chunk in sorted(set(re.findall(r"/_next/static/[a-zA-Z0-9_\-/.]+\.js", page))):
    body = fetch(SITE + chunk)
    match = re.search(r"JSON\.parse\('(\[\{\"droprate.*?)'\)", body, re.S)
    if match:
        items = json.loads(match.group(1).replace("\\'", "'"))
        break

if not items:
    raise SystemExit("the item table is no longer in the bundles — check how the site ships its data")

OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(json.dumps(items, ensure_ascii=False), encoding="utf-8")
print(f"{len(items)} items -> {OUT}")
