#!/usr/bin/env python3
"""ANSI (tmux `capture-pane -e`) → PNG, one cell per glyph, with Pillow.

Understands the SGR codes nebula emits through crossterm: reset, bold, dim, underline, reverse, 16
colours, 256-colour and truecolour foreground/background. Everything else is ignored, never fatal.
    python3 render.py capture.ansi out.png [--cols 190] [--rows 50] [--font /System/Library/Fonts/Menlo.ttc] [--size 13]
"""
import argparse
import re
import sys

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    sys.exit("render.py needs Pillow: python3 -m pip install pillow (shot.sh makes a venv for this)")

ANSI16 = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
    (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
]
SGR = re.compile(r"\x1b\[([0-9;]*)m")
OTHER_ESC = re.compile(r"\x1b\][^\x07]*\x07|\x1b\[[0-9;?]*[A-Za-z]|\x1b[()][A-Za-z0-9]")
DEFAULT_FG = (204, 204, 204)
DEFAULT_BG = (18, 18, 18)


def xterm256(n):
    if n < 16:
        return ANSI16[n]
    if n < 232:
        n -= 16
        r, g, b = n // 36, (n // 6) % 6, n % 6
        return tuple(0 if v == 0 else 55 + v * 40 for v in (r, g, b))
    v = 8 + (n - 232) * 10
    return (v, v, v)


def parse(text, cols, rows):
    """Rows of (char, fg, bg, bold, dim, underline) cells."""
    grid = []
    state = dict(fg=None, bg=None, bold=False, dim=False, ul=False, rev=False)
    for line in text.split("\n")[:rows]:
        cells = []
        i = 0
        while i < len(line) and len(cells) < cols:
            if line[i] == "\x1b":
                m = SGR.match(line, i)
                if m:
                    codes = [int(c) if c else 0 for c in m.group(1).split(";")] or [0]
                    j = 0
                    while j < len(codes):
                        c = codes[j]
                        if c == 0:
                            state.update(fg=None, bg=None, bold=False, dim=False, ul=False, rev=False)
                        elif c == 1: state["bold"] = True
                        elif c == 2: state["dim"] = True
                        elif c == 4: state["ul"] = True
                        elif c == 7: state["rev"] = True
                        elif c == 22: state["bold"] = state["dim"] = False
                        elif c == 24: state["ul"] = False
                        elif c == 27: state["rev"] = False
                        elif 30 <= c <= 37: state["fg"] = ANSI16[c - 30]
                        elif 90 <= c <= 97: state["fg"] = ANSI16[c - 90 + 8]
                        elif 40 <= c <= 47: state["bg"] = ANSI16[c - 40]
                        elif 100 <= c <= 107: state["bg"] = ANSI16[c - 100 + 8]
                        elif c == 39: state["fg"] = None
                        elif c == 49: state["bg"] = None
                        elif c in (38, 48) and j + 1 < len(codes):
                            key = "fg" if c == 38 else "bg"
                            if codes[j + 1] == 5 and j + 2 < len(codes):
                                state[key] = xterm256(codes[j + 2]); j += 2
                            elif codes[j + 1] == 2 and j + 4 < len(codes):
                                state[key] = tuple(codes[j + 2:j + 5]); j += 4
                        j += 1
                    i = m.end()
                    continue
                m = OTHER_ESC.match(line, i)
                i = m.end() if m else i + 1
                continue
            ch = line[i]
            fg = state["fg"] or DEFAULT_FG
            bg = state["bg"] or DEFAULT_BG
            if state["rev"]:
                fg, bg = bg, fg
            cells.append((ch, fg, bg, state["bold"], state["dim"], state["ul"]))
            i += 1
        while len(cells) < cols:
            cells.append((" ", DEFAULT_FG, DEFAULT_BG, False, False, False))
        grid.append(cells)
    while len(grid) < rows:
        grid.append([(" ", DEFAULT_FG, DEFAULT_BG, False, False, False)] * cols)
    return grid


def render(grid, out, font_path, size):
    font = ImageFont.truetype(font_path, size)
    try:
        bold = ImageFont.truetype(font_path, size, index=1)
    except Exception:
        bold = font
    cw = int(round(font.getlength("M")))
    ch = int(round(size * 1.35))
    rows, cols = len(grid), len(grid[0])
    img = Image.new("RGB", (cols * cw, rows * ch), DEFAULT_BG)
    draw = ImageDraw.Draw(img)
    for y, row in enumerate(grid):
        skip = False
        for x, (c, fg, bg, b, dim, ul) in enumerate(row):
            if skip:
                skip = False
                continue
            if bg != DEFAULT_BG:
                draw.rectangle([x * cw, y * ch, (x + 1) * cw - 1, (y + 1) * ch - 1], fill=bg)
            if c == " ":
                continue
            color = tuple(v // 2 for v in fg) if dim else fg
            draw.text((x * cw, y * ch + 2), c, font=bold if b else font, fill=color)
            if ul:
                draw.line([x * cw, (y + 1) * ch - 2, (x + 1) * cw, (y + 1) * ch - 2], fill=color)
            if ord(c) > 0x2E7F and font.getlength(c) > cw * 1.5:
                skip = True  # a double-width glyph took two cells
    img.save(out)
    return img.size


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ansi"); ap.add_argument("png")
    ap.add_argument("--cols", type=int, default=190); ap.add_argument("--rows", type=int, default=50)
    ap.add_argument("--font", default="/System/Library/Fonts/Menlo.ttc"); ap.add_argument("--size", type=int, default=13)
    a = ap.parse_args()
    text = open(a.ansi, encoding="utf-8", errors="replace").read()
    size = render(parse(text, a.cols, a.rows), a.png, a.font, a.size)
    print("%s %dx%d" % (a.png, *size))


if __name__ == "__main__":
    main()
