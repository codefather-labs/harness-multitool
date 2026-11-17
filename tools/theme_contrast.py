#!/usr/bin/env python3
"""CON-180: the contrast of each theme's colours, by WCAG 2.x relative luminance, against the floors
the shipped light theme states in its header. Prints every pair; exits 1 when a theme named on the
command line (all of themes/*.toml when none is named) falls below a floor.
Usage: tools/theme_contrast.py [id ...]"""
import glob, os, sys, tomllib

FLOORS = [
    ("text", "background", 7.0), ("text", "surface", 7.0),
    ("text_muted", "background", 4.5), ("text_muted", "surface_raised", 4.5),
    ("accent_text", "accent", 4.5), ("accent", "background", 4.5),
    ("success", "background", 4.5), ("warning", "background", 4.5), ("error", "background", 4.5), ("info", "background", 4.5),
    ("agent", "background", 4.5),
    ("accent_line", "background", 3.0), ("accent_line", "surface", 3.0),
]

def lum(hexcolour):
    h = hexcolour.lstrip("#")
    def ch(v):
        c = int(v, 16) / 255
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (ch(h[i:i + 2]) for i in (0, 2, 4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b

def ratio(a, b):
    la, lb = sorted((lum(a), lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)

here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "themes")
wanted = sys.argv[1:] or [os.path.basename(p)[:-5] for p in sorted(glob.glob(os.path.join(here, "*.toml")))]
failed = []
for tid in wanted:
    t = tomllib.load(open(os.path.join(here, f"{tid}.toml"), "rb"))
    t.setdefault("accent_line", t["accent"])
    rows = []
    for fg, bg, floor in FLOORS:
        if fg not in t or bg not in t:
            continue
        r = ratio(t[fg], t[bg])
        ok = r >= floor
        rows.append(f"  {fg:<12} on {bg:<15} {r:5.2f}:1  (floor {floor}){'' if ok else '  BELOW'}")
        if not ok:
            failed.append(f"{tid}: {fg} on {bg} {r:.2f} < {floor}")
    print(f"{tid} ({t.get('kind')})"); print("\n".join(rows))
print()
print("ALL ABOVE THEIR FLOORS" if not failed else "BELOW A FLOOR:\n" + "\n".join(failed))
sys.exit(1 if failed else 0)
