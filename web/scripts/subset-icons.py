#!/usr/bin/env python3
"""Builds the Material Symbols Outlined subset the UI ships.

Reads the icon names from src/ui/icon-names.ts, keeps only those ligatures (and their
filled variants), pins weight 400, grade 0 and optical size 24, keeps the FILL axis and
writes src/assets/fonts/material-symbols.woff2 plus a manifest that `pnpm build` checks.

Needs Python 3 with fonttools and brotli: `pip install fonttools brotli`.
Run with `pnpm icons` after adding an icon name.
"""
import io
import json
import pathlib
import re
import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = pathlib.Path(__file__).resolve().parent.parent
NAMES_FILE = ROOT / "src/ui/icon-names.ts"
SOURCE = ROOT / "node_modules/@fontsource-variable/material-symbols-outlined/files/material-symbols-outlined-latin-full-normal.woff2"
OUT_DIR = ROOT / "src/assets/fonts"


def icon_names() -> list[str]:
    body = NAMES_FILE.read_text()
    block = body[body.index("[") : body.rindex("]")]
    return sorted(set(re.findall(r'"([a-z0-9_]+)"', block)))


def ligature_map(font: TTFont) -> dict[str, str]:
    """Maps each ligature's text ("favorite") to the glyph it produces."""
    chars = {}
    for code, glyph in font.getBestCmap().items():
        if 97 <= code <= 122 or 48 <= code <= 57 or code == 95:
            chars.setdefault(glyph, chr(code))
    found: dict[str, str] = {}
    for lookup in font["GSUB"].table.LookupList.Lookup:
        for table in lookup.SubTable:
            if lookup.LookupType == 7:
                table = table.ExtSubTable
            if getattr(table, "LookupType", lookup.LookupType) != 4:
                continue
            for first, ligatures in table.ligatures.items():
                for lig in ligatures:
                    parts = [first, *lig.Component]
                    if all(p in chars for p in parts):
                        found.setdefault("".join(chars[p] for p in parts), lig.LigGlyph)
    return found


def main() -> None:
    names = icon_names()
    font = TTFont(SOURCE)
    ligatures = ligature_map(font)
    missing = [n for n in names if n not in ligatures]
    if missing:
        sys.exit(f"Not in Material Symbols Outlined: {', '.join(missing)}")
    order = set(font.getGlyphOrder())
    glyphs = []
    for name in names:
        glyph = ligatures[name]
        glyphs.append(glyph)
        if f"{glyph}.fill" in order:
            glyphs.append(f"{glyph}.fill")

    options = subset.Options()
    options.layout_features = ["*"]
    options.layout_closure = False
    options.name_IDs = [1, 2]
    options.name_languages = [0x409]
    options.notdef_outline = True
    options.glyph_names = False
    options.flavor = "woff2"
    letters = sorted({ord(c) for c in "".join(names)} | {ord(c.upper()) for c in "".join(names) if c.isalpha()})
    subsetter = subset.Subsetter(options)
    subsetter.populate(glyphs=glyphs, unicodes=letters)
    subsetter.subset(font)
    # Instance after subsetting: pinning axes first trips fontTools on lazily loaded gvar data.
    buffer = io.BytesIO()
    font.flavor = None
    font.save(buffer)
    buffer.seek(0)
    font = instancer.instantiateVariableFont(TTFont(buffer), {"wght": 400, "GRAD": 0, "opsz": 24})
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / "material-symbols.woff2"
    font.flavor = "woff2"
    font.save(out)
    (OUT_DIR / "material-symbols.json").write_text(json.dumps({"icons": names}, indent=1) + "\n")
    print(f"{len(names)} icons, {out.stat().st_size / 1024:.1f} KB -> {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
