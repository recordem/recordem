#!/usr/bin/env python3
"""Turns a `probe dump` into a fixture: only the call window's web content (no tabs, bookmarks, other windows),
names and meeting codes replaced. Check the result for anything personal before committing it.

    redact.py dump.json fixture.json "Real Name=Ana Souza" "abc-defg-hij=xyz-wxyz-xyz" ...
"""
import json, sys

src, dst, *pairs = sys.argv[1:]
subs = [p.split("=", 1) for p in pairs]


def walk(n):
    yield n
    for c in n.get("children", []):
        yield from walk(c)


def red(s):
    for a, b in subs:
        s = s.replace(a, b)
    return s


tree = json.load(open(src))
windows = []
for w in tree.get("children", []):
    if w.get("role") != "AXWindow":
        continue
    call = (w.get("title") or "").startswith("Meet")
    # Pages of other windows are not the call's: left out.
    web = [n for n in walk(w) if n.get("role") == "AXWebArea"] if call else []
    for n in web:
        for m in walk(n):
            for k in ("title", "description", "value"):
                if k in m:
                    m[k] = red(m[k])
    # The tab strip, for calls in a background tab: only call tabs keep their title.
    tabs = [
        {"role": "AXRadioButton", "subrole": "AXTabButton",
         "title": red(n["title"]) if (n.get("title") or "").startswith("Meet") else "Other tab"}
        for n in walk(w) if n.get("subrole") == "AXTabButton"
    ]
    strip = [{"role": "AXTabGroup", "children": tabs}] if tabs else []
    title = w.get("title") or ""
    title = red(title) if title.startswith("Meet") else "Other window"
    windows.append({"role": "AXWindow", "title": title, "frame": w.get("frame"), "children": strip + web})
json.dump({"role": "AXApplication", "children": windows}, open(dst, "w"), indent=1, ensure_ascii=False)
