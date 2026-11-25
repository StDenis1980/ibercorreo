#!/usr/bin/env python3
"""Tinta Office: translations of the desktop entry and the AppStream metadata.

The release's desktop entry and metainfo template stay in English and unchanged, so upstream
changes to them merge cleanly. Their translations live in tinta-l10n.txt next to this script:
one row per language and English text, keyed by the release's English exactly as it is in those
files (before tinta-brand.sh renames the product), so a changed English text shows up as a
missing translation instead of a stale one.

    tinta-l10n.py apply DESKTOP METAINFO   add the translations to staged copies, in place
    tinta-l10n.py check DESKTOP METAINFO   list missing and unused rows; exit 1 if any is missing

Translated: the desktop entry's GenericName, Comment and Keywords, and the metainfo's summary,
description paragraphs, screenshot captions and keywords. The product name is a brand and is not
translated. `apply` leaves a missing translation in English and only warns, so a release never
fails on one; `check` is what tests run.
"""

import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape, unescape

# Every Tinta app ships every language in tinta-languages.txt (tintaoffice-web#43).
LANGUAGES = [
    line.strip()
    for line in Path(__file__).with_name("tinta-languages.txt").read_text(encoding="utf-8").splitlines()
    if line.strip() and not line.startswith("#")
]
DESKTOP_KEYS = ("GenericName", "Comment", "Keywords")
TSV = Path(__file__).with_name("tinta-l10n.txt")


def load(path):
    rows = {}
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) != 3 or not all(p.strip() for p in parts):
            sys.exit(f"error: {path}:{number}: expected language<TAB>English<TAB>translation")
        lang, source, text = parts
        if lang not in LANGUAGES:
            sys.exit(f"error: {path}:{number}: unknown language {lang!r}")
        if (lang, source) in rows:
            sys.exit(f"error: {path}:{number}: {lang} {source!r} is given twice")
        rows[(lang, source)] = text
    return rows


def plain(xml_text):
    return " ".join(unescape(xml_text).split())


def desktop_sources(text):
    sources = []
    for line in text.splitlines():
        if line.startswith("[") and line != "[Desktop Entry]":
            break
        key, _, value = line.partition("=")
        if key in DESKTOP_KEYS:
            sources.append(value)
    return sources


# The English elements of the metainfo: (regex match, kind). Only elements without xml:lang.
SIMPLE = re.compile(r"^([ \t]*)<(summary|caption)>(.*?)</\2>[ \t]*$", re.M)
PARA = re.compile(r"^([ \t]*)<p>(.*?)</p>[ \t]*$", re.M | re.S)
KEYWORDS = re.compile(r"^([ \t]*)<keywords>(.*?)^([ \t]*)</keywords>", re.M | re.S)
KEYWORD = re.compile(r"<keyword>(.*?)</keyword>")


def keyword_source(block):
    return ";".join(plain(k) for k in KEYWORD.findall(block)) + ";"


def metainfo_sources(text):
    sources = [plain(m.group(3)) for m in SIMPLE.finditer(text)]
    description = re.search(r"<description>(.*?)</description>", text, re.S)
    if description:
        sources += [plain(m.group(2)) for m in PARA.finditer(description.group(1))]
    sources += [keyword_source(m.group(2)) for m in KEYWORDS.finditer(text)]
    return sources


def sources_of(desktop, metainfo):
    return desktop_sources(desktop.read_text(encoding="utf-8")) + metainfo_sources(
        metainfo.read_text(encoding="utf-8")
    )


def check(rows, sources):
    missing = [(lang, s) for s in sources for lang in LANGUAGES if (lang, s) not in rows]
    unused = sorted({s for (_, s) in rows} - set(sources))
    return missing, unused


def apply_desktop(path, rows):
    out = []
    in_entry = False
    for line in path.read_text(encoding="utf-8").splitlines():
        out.append(line)
        if line.startswith("["):
            in_entry = line == "[Desktop Entry]"
            continue
        key, _, value = line.partition("=")
        if in_entry and key in DESKTOP_KEYS:
            for lang in LANGUAGES:
                if (lang, value) in rows:
                    out.append(f"{key}[{lang}]={rows[(lang, value)]}")
    path.write_text("\n".join(out) + "\n", encoding="utf-8")


def apply_metainfo(path, rows):
    text = path.read_text(encoding="utf-8")

    def translations(indent, tag, source):
        return "".join(
            f'\n{indent}<{tag} xml:lang="{lang}">{escape(rows[(lang, source)])}</{tag}>'
            for lang in LANGUAGES
            if (lang, source) in rows
        )

    def simple(m):
        return m.group(0) + translations(m.group(1), m.group(2), plain(m.group(3)))

    def para(m):
        return m.group(0) + translations(m.group(1), "p", plain(m.group(2)))

    def description(m):
        return PARA.sub(para, m.group(0))

    def keywords(m):
        source = keyword_source(m.group(2))
        indent = m.group(1) + "  "
        extra = ""
        for lang in LANGUAGES:
            if (lang, source) in rows:
                for word in rows[(lang, source)].split(";"):
                    if word.strip():
                        extra += f'{indent}<keyword xml:lang="{lang}">{escape(word.strip())}</keyword>\n'
        return f"{m.group(1)}<keywords>{m.group(2)}{extra}{m.group(3)}</keywords>"

    text = SIMPLE.sub(simple, text)
    text = re.sub(r"<description>.*?</description>", description, text, flags=re.S)
    text = KEYWORDS.sub(keywords, text)
    path.write_text(text, encoding="utf-8")


def main(argv):
    if len(argv) != 4 or argv[1] not in ("apply", "check"):
        sys.exit(__doc__)
    desktop, metainfo = Path(argv[2]), Path(argv[3])
    rows = load(TSV)
    missing, unused = check(rows, sources_of(desktop, metainfo))
    for lang, source in missing:
        print(f"missing translation: {lang}\t{source}", file=sys.stderr)
    for source in unused:
        print(f"unused translation (English changed?): {source}", file=sys.stderr)
    if argv[1] == "check":
        return 1 if missing else 0
    apply_desktop(desktop, rows)
    apply_metainfo(metainfo, rows)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
