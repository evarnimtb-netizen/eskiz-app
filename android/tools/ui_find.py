#!/usr/bin/env python3
"""Find a node in a `uiautomator dump` XML and print the centre of its bounds as "X Y" (standard library only).

usage: ui_find.py DUMP.xml (--text T | --text-contains T | --id ID | --desc D) [--index N] [--package P]

Matching is exact for --text, --id (resource-id, with or without the "pkg:id/" prefix) and --desc
(content-desc); --text-contains is a case-insensitive substring match against text and content-desc.
--index picks the Nth match (default 0, in document order). Exit 0 and one line on success; exit 1 and
nothing on stdout when no node matches (a message goes to stderr).
"""
import re
import sys
import xml.etree.ElementTree as ET

BOUNDS = re.compile(r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]")


def centre(bounds):
    m = BOUNDS.fullmatch(bounds or "")
    if not m:
        return None
    x0, y0, x1, y1 = (int(v) for v in m.groups())
    if x1 <= x0 or y1 <= y0:
        return None
    return (x0 + x1) // 2, (y0 + y1) // 2


def find(xml_text, text=None, contains=None, rid=None, desc=None, package=None):
    """All matching centres, in document order."""
    # uiautomator may print "UI hierchary dumped to: ..." after the XML.
    end = xml_text.rfind("</hierarchy>")
    if end >= 0:
        xml_text = xml_text[: end + len("</hierarchy>")]
    root = ET.fromstring(xml_text)
    out = []
    for node in root.iter("node"):
        a = node.attrib
        if package and a.get("package") != package:
            continue
        if text is not None and a.get("text") != text:
            continue
        if desc is not None and a.get("content-desc") != desc:
            continue
        if rid is not None:
            r = a.get("resource-id", "")
            if r != rid and not r.endswith("/" + rid):
                continue
        if contains is not None:
            needle = contains.lower()
            if needle not in a.get("text", "").lower() and needle not in a.get("content-desc", "").lower():
                continue
        c = centre(a.get("bounds"))
        if c:
            out.append(c)
    return out


def main(argv):
    opts = {"text": None, "contains": None, "rid": None, "desc": None, "package": None}
    index = 0
    path = None
    i = 0
    flags = {"--text": "text", "--text-contains": "contains", "--id": "rid", "--desc": "desc", "--package": "package"}
    while i < len(argv):
        a = argv[i]
        if a in flags and i + 1 < len(argv):
            opts[flags[a]] = argv[i + 1]
            i += 2
        elif a == "--index" and i + 1 < len(argv):
            index = int(argv[i + 1])
            i += 2
        elif path is None and not a.startswith("--"):
            path = a
            i += 1
        else:
            raise SystemExit(__doc__)
    if path is None or all(opts[k] is None for k in ("text", "contains", "rid", "desc")):
        raise SystemExit(__doc__)
    try:
        found = find(open(path, encoding="utf-8", errors="replace").read(), **opts)
    except (ET.ParseError, OSError) as e:
        print(f"ui_find: cannot read {path}: {e}", file=sys.stderr)
        return 2
    if index >= len(found):
        print(f"ui_find: no node for {opts} (index {index}, {len(found)} matches)", file=sys.stderr)
        return 1
    print(f"{found[index][0]} {found[index][1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
