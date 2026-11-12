#!/usr/bin/env python3
import json
import math
import subprocess
import sys
import tempfile
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONT_DIR = Path("/usr/share/fonts/TTF")
FACES = {
    "": "JetBrainsMonoNerdFontMono-Regular.ttf",
    "b": "JetBrainsMonoNerdFontMono-Bold.ttf",
    "i": "JetBrainsMonoNerdFontMono-Italic.ttf",
    "bi": "JetBrainsMonoNerdFontMono-BoldItalic.ttf",
}
SYMBOL_FACE = FONT_DIR / "DejaVuSansMNerdFontMono-Regular.ttf"
EMOJI_FACE = Path("/usr/share/fonts/noto/NotoColorEmoji.ttf")
EMOJI_STRIKE = 109

FONT_SIZE = 18
CELL_W = 11
CELL_H = 23
PAD = 14
BAR_H = 34
MARGIN_X = 20
MARGIN_TOP = 18
MARGIN_BOTTOM = 54
RADIUS = 12
LINE = 1
DIM = 0.55
TICK = 30
ART_TICK = 60
BLINK_MS = 1080
ANIMATED = {"sticker": 1.2, "clip": 2.4}
SUPER = 3

LIGHT = {
    "─": "lr", "│": "ud", "┌": "rd", "┐": "ld", "└": "ru", "┘": "lu",
    "├": "udr", "┤": "udl", "┬": "lrd", "┴": "lru", "┼": "lrud",
    "╭": "rd", "╮": "ld", "╰": "ru", "╯": "lu",
    "╴": "l", "╵": "u", "╶": "r", "╷": "d",
}
LOWER = "▁▂▃▄▅▆▇█"
LEFT = "▏▎▍▌▋▊▉"


def rgb(value):
    return tuple(int(value[i:i + 2], 16) for i in (0, 2, 4))


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def ease(t):
    t = min(1.0, max(0.0, t))
    return t * t * (3 - 2 * t)


class Fonts:
    def __init__(self):
        self.faces = {
            key: ImageFont.truetype(str(FONT_DIR / name), FONT_SIZE)
            for key, name in FACES.items()
        }
        self.cmap = TTFont(str(FONT_DIR / FACES[""])).getBestCmap()
        self.symbols = ImageFont.truetype(str(SYMBOL_FACE), FONT_SIZE)
        self.symbol_cmap = TTFont(str(SYMBOL_FACE)).getBestCmap()
        self.emoji = ImageFont.truetype(str(EMOJI_FACE), EMOJI_STRIKE)
        self.emoji_cmap = TTFont(str(EMOJI_FACE)).getBestCmap()
        self.emoji_cache = {}
        ascent, descent = self.faces[""].getmetrics()
        self.baseline = (CELL_H - ascent - descent) // 2 + ascent

    def sized(self, style, size):
        return ImageFont.truetype(str(FONT_DIR / FACES[style]), size)

    def emoji_image(self, symbol):
        if symbol not in self.emoji_cache:
            box = self.emoji.getbbox(symbol)
            tile = Image.new("RGBA", (box[2], box[3]), (0, 0, 0, 0))
            ImageDraw.Draw(tile).text((0, 0), symbol, font=self.emoji, embedded_color=True)
            tile = tile.crop(tile.getbbox())
            side = min(2 * CELL_W, CELL_H) - 2
            tile.thumbnail((side, side), Image.LANCZOS)
            self.emoji_cache[symbol] = tile
        return self.emoji_cache[symbol]

    def pick(self, symbol, style):
        codes = [ord(ch) for ch in symbol if ch not in "️‍"]
        if all(code in self.cmap for code in codes):
            return self.faces[style]
        if all(code in self.emoji_cmap for code in codes):
            return None
        if all(code in self.symbol_cmap for code in codes):
            return self.symbols
        raise SystemExit(f"no font has a glyph for {symbol!r}")


def draw_lines(draw, symbol, x, y, color):
    arms = LIGHT[symbol]
    cx = x + CELL_W // 2
    cy = y + CELL_H // 2
    if "l" in arms:
        draw.rectangle((x, cy, cx, cy + LINE - 1), fill=color)
    if "r" in arms:
        draw.rectangle((cx, cy, x + CELL_W - 1, cy + LINE - 1), fill=color)
    if "u" in arms:
        draw.rectangle((cx, y, cx + LINE - 1, cy), fill=color)
    if "d" in arms:
        draw.rectangle((cx, cy, cx + LINE - 1, y + CELL_H - 1), fill=color)


def draw_block(draw, symbol, x, y, color):
    right = x + CELL_W - 1
    bottom = y + CELL_H - 1
    if symbol in LOWER:
        height = round(CELL_H * (LOWER.index(symbol) + 1) / 8)
        draw.rectangle((x, bottom - height + 1, right, bottom), fill=color)
    elif symbol in LEFT:
        width = max(1, round(CELL_W * (LEFT.index(symbol) + 1) / 8))
        draw.rectangle((x, y, x + width - 1, bottom), fill=color)
    elif symbol == "▐":
        draw.rectangle((x + CELL_W // 2, y, right, bottom), fill=color)
    elif symbol == "▀":
        draw.rectangle((x, y, right, y + CELL_H // 2 - 1), fill=color)
    else:
        return False
    return True


def draw_cell(draw, fonts, col, row, cell):
    symbol, fg, bg, flags = cell
    fg, bg = rgb(fg), rgb(bg)
    if "r" in flags:
        fg, bg = bg, fg
    if "d" in flags:
        fg = mix(bg, fg, DIM)
    x = col * CELL_W
    y = row * CELL_H
    draw.rectangle((x, y, x + CELL_W - 1, y + CELL_H - 1), fill=bg)
    if symbol in ("", " "):
        return None
    if symbol in LIGHT:
        draw_lines(draw, symbol, x, y, fg)
        return None
    if draw_block(draw, symbol, x, y, fg):
        return None
    style = ("b" if "b" in flags else "") + ("i" if "i" in flags else "")
    font = fonts.pick(symbol, style)
    if font is None:
        return (x, y, symbol)
    draw.text((x, y + fonts.baseline), symbol, font=font, fill=fg, anchor="ls")
    if "u" in flags:
        draw.rectangle((x, y + fonts.baseline + 2, x + CELL_W - 1, y + fonts.baseline + 2), fill=fg)
    if "s" in flags:
        draw.rectangle((x, y + CELL_H // 2, x + CELL_W - 1, y + CELL_H // 2), fill=fg)
    return None


def half_box(col, half):
    x = col * CELL_W
    y = (half // 2) * CELL_H
    split = CELL_H // 2
    if half % 2 == 0:
        return (x, y, x + CELL_W - 1, y + split - 1)
    return (x, y + split, x + CELL_W - 1, y + CELL_H - 1)


def fit_axis(samples):
    xs = sorted({x for x, _ in samples})
    if len(xs) < 2:
        return None
    n = len(samples)
    mean_x = sum(x for x, _ in samples) / n
    mean_y = sum(y for _, y in samples) / n
    var = sum((x - mean_x) ** 2 for x, _ in samples)
    slope = sum((x - mean_x) * (y - mean_y) for x, y in samples) / var
    if slope <= 0:
        return None
    start = -(mean_y - slope * mean_x) / slope
    span = 1 / slope
    if abs(start - round(start)) < 0.25:
        start = round(start)
    if abs(span - round(span)) < 0.35:
        span = round(span)
    return start, span


def placements(rows, art, size):
    found = {}
    for r, row in enumerate(rows):
        for c, cell in enumerate(row):
            symbol = cell[0]
            if symbol not in ("▄", "▀"):
                continue
            fg, bg = rgb(cell[1]), rgb(cell[2])
            if fg[0] != bg[0] or str(fg[0]) not in art:
                continue
            top, bottom = (bg, fg) if symbol == "▄" else (fg, bg)
            entry = found.setdefault(fg[0], {"u": [], "v": [], "halves": [], "first": None})
            if entry["first"] is None:
                entry["first"] = (c, r)
            for half, color in ((2 * r, top), (2 * r + 1, bottom)):
                entry["u"].append((c + 0.5, color[1] / 255))
                entry["v"].append((half + 0.5, color[2] / 255))
                entry["halves"].append((c, half))
    out = []
    for key, entry in found.items():
        mask = Image.new("L", size, 0)
        pen = ImageDraw.Draw(mask)
        for c, half in entry["halves"]:
            pen.rectangle(half_box(c, half), fill=255)
        c, r = entry["first"]
        matte = rgb(rows[r][c - 1][2]) if c > 0 else None
        across, down = fit_axis(entry["u"]), fit_axis(entry["v"])
        rect = None
        if across and down:
            rect = (
                round(across[0] * CELL_W),
                round(down[0] * CELL_H / 2),
                max(1, round(across[1] * CELL_W)),
                max(1, round(down[1] * CELL_H / 2)),
            )
        out.append({"key": str(key), "mask": mask, "rect": rect, "matte": matte})
    return out


class Sketch:
    def __init__(self, w, h, fill=(0, 0, 0, 0)):
        self.w, self.h = w, h
        self.image = Image.new("RGBA", (w * SUPER, h * SUPER), fill)
        self.pen = ImageDraw.Draw(self.image)

    def pt(self, x, y):
        return (x * self.w * SUPER, y * self.h * SUPER)

    def poly(self, points, fill):
        self.pen.polygon([self.pt(x, y) for x, y in points], fill=fill)

    def rect(self, x0, y0, x1, y1, fill):
        self.pen.rectangle((*self.pt(x0, y0), *self.pt(x1, y1)), fill=fill)

    def oval(self, cx, cy, rx, ry, fill):
        x0, y0 = self.pt(cx - rx, cy - ry)
        x1, y1 = self.pt(cx + rx, cy + ry)
        self.pen.ellipse((x0, y0, x1, y1), fill=fill)

    def disc(self, cx, cy, r, fill):
        self.oval(cx, cy, r * self.h / self.w, r, fill)

    def stroke(self, points, width, fill):
        scaled = [self.pt(x, y) for x, y in points]
        self.pen.line(scaled, fill=fill, width=max(1, round(width * self.h * SUPER)), joint="curve")
        radius = width * self.h * SUPER / 2
        for x, y in (scaled[0], scaled[-1]):
            self.pen.ellipse((x - radius, y - radius, x + radius, y + radius), fill=fill)

    def bands(self, top, bottom, colors):
        step = (bottom - top) / len(colors)
        for i, color in enumerate(colors):
            self.rect(0, top + i * step, 1, top + (i + 1) * step + 0.01, color)

    def ridge(self, base, height, seed, fill, floor=1.0):
        points = [(0, floor)]
        for i in range(61):
            x = i / 60
            wave = (
                math.sin(x * 7.1 + seed) * 0.5
                + math.sin(x * 15.3 + seed * 2.3) * 0.3
                + math.sin(x * 31.7 + seed * 0.7) * 0.2
            )
            points.append((x, base - height * (0.5 + 0.5 * wave)))
        points.append((1, floor))
        self.poly(points, fill)

    def pine(self, x, base, height, fill):
        half = height * 0.22 * self.h / self.w
        for k in range(3):
            top = base - height + k * height * 0.22
            bottom = base - height * 0.45 + k * height * 0.27
            spread = half * (0.55 + 0.25 * k)
            self.poly([(x, top), (x - spread, bottom), (x + spread, bottom)], fill)
        self.rect(x - half * 0.12, base - height * 0.2, x + half * 0.12, base, fill)

    def done(self):
        return self.image.resize((self.w, self.h), Image.LANCZOS)


def art_mountains(w, h, _t, _spec, _fonts):
    s = Sketch(w, h, (0, 0, 0, 255))
    s.bands(0, 0.66, [
        (64, 92, 168), (84, 112, 186), (112, 136, 204), (150, 160, 216),
        (196, 178, 214), (236, 196, 196), (250, 214, 176),
    ])
    s.disc(0.74, 0.33, 0.085, (255, 240, 200))
    s.disc(0.74, 0.33, 0.062, (255, 250, 228))
    s.ridge(0.56, 0.2, 1.3, (138, 146, 204))
    s.ridge(0.64, 0.22, 4.1, (104, 114, 178))
    s.poly([(0.12, 0.74), (0.38, 0.2), (0.66, 0.74)], (72, 84, 146))
    s.poly([(0.38, 0.2), (0.66, 0.74), (0.45, 0.74)], (56, 66, 122))
    s.poly([
        (0.38, 0.2), (0.31, 0.345), (0.345, 0.32), (0.372, 0.375), (0.405, 0.325),
        (0.44, 0.385), (0.468, 0.35), (0.47, 0.378),
    ], (240, 244, 252))
    s.poly([(0.38, 0.2), (0.405, 0.325), (0.44, 0.385), (0.468, 0.35)], (206, 214, 236))
    s.poly([(0.6, 0.78), (0.82, 0.4), (1.0, 0.62), (1.0, 0.78)], (64, 76, 136))
    s.poly([(0.82, 0.4), (0.78, 0.47), (0.815, 0.455), (0.84, 0.49), (0.868, 0.458)], (232, 238, 250))
    s.ridge(0.8, 0.12, 2.2, (44, 84, 96))
    s.ridge(0.9, 0.12, 5.6, (30, 62, 74))
    s.poly([(0.44, 1.0), (0.5, 0.88), (0.47, 0.8), (0.5, 0.76), (0.52, 0.8), (0.56, 0.88), (0.6, 1.0)], (214, 196, 160))
    for i, x in enumerate([0.04, 0.11, 0.19, 0.27, 0.34, 0.68, 0.75, 0.83, 0.9, 0.97]):
        s.pine(x, 1.02 - (i % 3) * 0.02, 0.2 + (i * 7 % 5) * 0.022, (18, 42, 52))
    return s.done()


def art_forest(w, h, _t, _spec, _fonts):
    s = Sketch(w, h, (0, 0, 0, 255))
    s.bands(0, 0.6, [(168, 214, 206), (188, 226, 208), (212, 236, 206), (236, 242, 204)])
    s.disc(0.2, 0.2, 0.09, (255, 246, 200))
    s.ridge(0.55, 0.2, 0.4, (120, 176, 150))
    s.ridge(0.68, 0.18, 3.0, (86, 146, 118))
    s.rect(0, 0.7, 1, 1, (64, 118, 92))
    for i in range(9):
        s.pine(0.06 + i * 0.115, 0.8 - (i % 2) * 0.03, 0.34 + (i * 5 % 4) * 0.03, (40, 96, 78))
    for i in range(7):
        s.pine(0.0 + i * 0.165, 1.04, 0.5 + (i * 3 % 4) * 0.04, (24, 66, 58))
    s.poly([(0.42, 1.0), (0.5, 0.84), (0.49, 0.74), (0.53, 0.74), (0.58, 0.84), (0.62, 1.0)], (206, 188, 144))
    return s.done()


def art_lake(w, h, _t, _spec, _fonts):
    s = Sketch(w, h, (0, 0, 0, 255))
    s.bands(0, 0.52, [(72, 70, 128), (124, 86, 142), (188, 104, 138), (236, 138, 120), (250, 184, 122)])
    s.disc(0.5, 0.52, 0.12, (255, 226, 160))
    s.ridge(0.5, 0.2, 2.6, (70, 58, 110), floor=0.52)
    s.poly([(0.0, 0.52), (0.0, 0.36), (0.14, 0.26), (0.3, 0.52)], (52, 44, 92))
    s.poly([(0.7, 0.52), (0.88, 0.22), (1.0, 0.36), (1.0, 0.52)], (52, 44, 92))
    s.bands(0.52, 1.0, [(206, 132, 122), (150, 98, 132), (104, 78, 128), (70, 62, 112), (50, 50, 96)])
    for i in range(7):
        y = 0.56 + i * 0.06
        half = 0.2 - i * 0.022
        s.rect(0.5 - half, y, 0.5 + half, y + 0.016, (255, 214, 160))
    s.poly([(0.0, 1.0), (0.0, 0.84), (0.2, 0.9), (0.34, 1.0)], (30, 30, 62))
    s.pine(0.08, 0.92, 0.36, (24, 24, 54))
    s.pine(0.17, 0.96, 0.26, (24, 24, 54))
    return s.done()


def art_camp(w, h, _t, _spec, _fonts):
    s = Sketch(w, h, (0, 0, 0, 255))
    s.bands(0, 0.7, [(22, 26, 58), (28, 34, 74), (36, 44, 92), (48, 58, 112)])
    for i in range(26):
        x = (i * 0.618 + 0.07) % 1
        y = (i * 0.377 + 0.05) % 0.5
        s.disc(x, y, 0.006 + (i % 3) * 0.003, (236, 240, 255))
    s.disc(0.82, 0.18, 0.08, (250, 244, 214))
    s.disc(0.79, 0.16, 0.07, (28, 34, 74))
    s.ridge(0.68, 0.2, 1.9, (26, 36, 70))
    s.rect(0, 0.76, 1, 1, (20, 30, 48))
    for i, x in enumerate([0.05, 0.14, 0.9, 0.97]):
        s.pine(x, 0.86, 0.4 + (i % 2) * 0.06, (14, 22, 38))
    s.poly([(0.22, 0.88), (0.42, 0.46), (0.62, 0.88)], (236, 150, 84))
    s.poly([(0.42, 0.46), (0.62, 0.88), (0.5, 0.88)], (196, 108, 62))
    s.poly([(0.42, 0.6), (0.36, 0.88), (0.47, 0.88)], (60, 40, 46))
    s.disc(0.76, 0.86, 0.075, (255, 150, 60))
    s.poly([(0.72, 0.88), (0.76, 0.68), (0.8, 0.88)], (255, 196, 80))
    s.poly([(0.74, 0.88), (0.765, 0.76), (0.785, 0.88)], (255, 240, 170))
    s.stroke([(0.69, 0.91), (0.83, 0.87)], 0.022, (92, 60, 44))
    s.stroke([(0.69, 0.87), (0.83, 0.91)], 0.022, (92, 60, 44))
    return s.done()


def art_sticker(w, h, t, _spec, _fonts):
    side = min(w, h)
    s = Sketch(side, side)
    beat = (t % 1.2) / 1.2
    hop = abs(math.sin(beat * math.pi * 2))
    lift = 0.07 * hop
    swing = math.sin(beat * math.pi * 4) * 0.5
    blink = 0.86 < beat < 0.93
    white = (255, 255, 255, 255)
    fur = (236, 164, 84, 255)
    dark = (190, 118, 58, 255)
    cream = (255, 226, 178, 255)
    ink = (52, 38, 44, 255)
    s.oval(0.5, 0.93, 0.26 - 0.05 * hop, 0.035, (0, 0, 0, 70))
    cy = 0.6 - lift
    hand = (0.86 + 0.05 * swing, cy - 0.33 - 0.03 * abs(swing))
    arm = [(0.7, cy - 0.02), hand]
    other = [(0.3, cy + 0.02), (0.17, cy + 0.14)]
    for grow, tone in ((0.035, white), (0, None)):
        s.disc(0.31, cy - 0.3, 0.085 + grow, tone or dark)
        s.disc(0.69, cy - 0.3, 0.085 + grow, tone or dark)
        s.stroke(arm, 0.1 + grow * 2, tone or dark)
        s.stroke(other, 0.1 + grow * 2, tone or dark)
        s.disc(hand[0], hand[1], 0.068 + grow, tone or fur)
        s.oval(0.5, cy + 0.08, 0.25 + grow, 0.24 + grow, tone or fur)
        s.disc(0.5, cy - 0.16, 0.235 + grow, tone or fur)
    s.disc(0.31, cy - 0.3, 0.045, cream)
    s.disc(0.69, cy - 0.3, 0.045, cream)
    s.oval(0.5, cy + 0.12, 0.15, 0.15, cream)
    s.oval(0.5, cy - 0.09, 0.12, 0.085, cream)
    s.oval(0.5, cy - 0.125, 0.04, 0.028, ink)
    if blink:
        s.stroke([(0.37, cy - 0.2), (0.43, cy - 0.2)], 0.018, ink)
        s.stroke([(0.57, cy - 0.2), (0.63, cy - 0.2)], 0.018, ink)
    else:
        s.disc(0.4, cy - 0.2, 0.032, ink)
        s.disc(0.6, cy - 0.2, 0.032, ink)
        s.disc(0.41, cy - 0.21, 0.011, white)
        s.disc(0.61, cy - 0.21, 0.011, white)
    smile = [(0.45 + i * 0.0125, cy - 0.075 + 0.028 * math.sin(i / 8 * math.pi)) for i in range(9)]
    s.stroke(smile, 0.016, ink)
    s.disc(0.33, cy - 0.11, 0.035, (246, 140, 130, 255))
    s.disc(0.67, cy - 0.11, 0.035, (246, 140, 130, 255))
    s.poly([(0.36, cy + 0.02), (0.64, cy + 0.02), (0.6, cy + 0.075), (0.4, cy + 0.075)], (86, 150, 230, 255))
    out = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    out.paste(s.done(), ((w - side) // 2, (h - side) // 2))
    return out


def art_clip(w, h, t, _spec, _fonts):
    s = Sketch(w, h, (24, 26, 38, 255))
    phase = (t % 2.4) / 2.4
    s.rect(0, 0, 1, 0.13, (36, 40, 58, 255))
    for i, tone in enumerate([(247, 118, 142), (224, 175, 104), (158, 206, 106)]):
        s.disc(0.045 + i * 0.045, 0.065, 0.028, (*tone, 255))
    s.rect(0.3, 0.045, 0.7, 0.085, (70, 78, 110, 255))
    targets = [0.34, 0.62, 0.48, 0.8, 0.56, 0.92]
    tones = [(122, 162, 247), (125, 207, 255), (187, 154, 247), (158, 206, 106), (224, 175, 104), (247, 118, 142)]
    for i, (target, tone) in enumerate(zip(targets, tones)):
        grow = ease((phase * 1.7 - i * 0.11) / 0.45) * (1 - ease((phase - 0.9) / 0.1))
        y = 0.22 + i * 0.115
        s.rect(0.06, y + 0.012, 0.2, y + 0.05, (70, 78, 110, 255))
        s.rect(0.24, y, 0.24 + 0.7 * target * grow + 0.004, y + 0.066, (*tone, 255))
    s.rect(0.06, 0.93, 0.94, 0.95, (52, 58, 84, 255))
    s.rect(0.06, 0.93, 0.06 + 0.88 * min(1, phase / 0.9), 0.95, (122, 162, 247, 255))
    return s.done()


ARTISTS = {
    "mountains": art_mountains,
    "forest": art_forest,
    "lake": art_lake,
    "camp": art_camp,
    "sticker": art_sticker,
    "clip": art_clip,
}


class Scene:
    def __init__(self, step, meta, fonts, cache):
        self.step = step
        self.meta = meta
        self.fonts = fonts
        self.cache = cache
        rows = step["rows"]
        self.size = (len(rows[0]) * CELL_W, len(rows) * CELL_H)
        self.base = Image.new("RGB", self.size, rgb(meta["bg"]))
        draw = ImageDraw.Draw(self.base)
        emoji = []
        for r, row in enumerate(rows):
            for c, cell in enumerate(row):
                if cell[0] == "" and c > 0:
                    cell = ["", row[c - 1][1], row[c - 1][2], row[c - 1][3]]
                late = draw_cell(draw, fonts, c, r, cell)
                if late:
                    emoji.append(late)
        for x, y, symbol in emoji:
            tile = fonts.emoji_image(symbol)
            at = (x + (2 * CELL_W - tile.width) // 2, y + (CELL_H - tile.height) // 2)
            self.base.paste(tile, at, tile)
        self.places = placements(rows, meta["art"], self.size)
        self.moving = any(meta["art"][p["key"]]["name"] in ANIMATED for p in self.places)
        self.still = None

    def artwork(self, place, t):
        spec = self.meta["art"][place["key"]]
        _, _, w, h = place["rect"]
        period = ANIMATED.get(spec["name"])
        tick = int((t % period) * 1000 / ART_TICK) if period else 0
        token = (place["key"], w, h, tick)
        if token not in self.cache:
            self.cache[token] = ARTISTS[spec["name"]](w, h, tick * ART_TICK / 1000, spec, self.fonts)
        return self.cache[token]

    def image(self, t):
        if self.still is not None:
            return self.still
        out = self.base.copy()
        for place in self.places:
            matte = place["matte"] or rgb(self.meta["bg"])
            layer = Image.new("RGB", self.size, matte)
            if place["rect"]:
                art = self.artwork(place, t)
                layer.paste(art, place["rect"][:2], art)
            out.paste(layer, (0, 0), place["mask"])
        if not self.moving:
            self.still = out
        return out


def pointer_sprite():
    scale = 4
    shape = [(1, 1), (1, 17), (5, 13), (8, 20), (11, 19), (8, 12), (13, 12)]
    big = Image.new("RGBA", (18 * scale, 24 * scale), (0, 0, 0, 0))
    pen = ImageDraw.Draw(big)
    points = [(x * scale, y * scale) for x, y in shape]
    pen.polygon(points, fill=(250, 250, 255, 255))
    pen.line(points + points[:1], fill=(16, 16, 24, 255), width=scale, joint="curve")
    arrow = big.resize((18, 24), Image.LANCZOS)
    shadow = Image.new("RGBA", (26, 32), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 110), (4, 5), arrow.split()[3])
    shadow = shadow.filter(ImageFilter.GaussianBlur(2))
    shadow.paste(arrow, (2, 2), arrow)
    return shadow


def fade(sprite, alpha):
    if alpha >= 1:
        return sprite
    out = sprite.copy()
    out.putalpha(sprite.split()[3].point(lambda v: round(v * alpha)))
    return out


def ripple(radius, alpha, color):
    scale = 3
    side = 2 * (radius + 3)
    big = Image.new("RGBA", (side * scale, side * scale), (0, 0, 0, 0))
    pen = ImageDraw.Draw(big)
    edge = 3 * scale
    pen.ellipse((edge, edge, side * scale - edge, side * scale - edge), outline=(*color, round(255 * alpha)), width=2 * scale)
    return big.resize((side, side), Image.LANCZOS)


class Stage:
    def __init__(self, meta, fonts, size):
        self.meta = meta
        self.fonts = fonts
        self.fg, self.bg = rgb(meta["fg"]), rgb(meta["bg"])
        self.accent = rgb(meta["accent"])
        self.term = size
        self.window = (size[0] + 2 * PAD, size[1] + 2 * PAD + BAR_H)
        self.size = (self.window[0] + 2 * MARGIN_X, self.window[1] + MARGIN_TOP + MARGIN_BOTTOM)
        self.origin = (MARGIN_X + PAD, MARGIN_TOP + BAR_H + PAD)
        self.backdrop = mix(self.bg, (0, 0, 0), 0.45)
        self.chrome = self.build_chrome()
        self.pointer = pointer_sprite()
        self.chip_font = fonts.sized("b", 17)
        self.chips = {}

    def build_chrome(self):
        scale = 2
        w, h = self.size
        shadow = Image.new("L", (w, h), 0)
        box = (MARGIN_X, MARGIN_TOP + 6, MARGIN_X + self.window[0], MARGIN_TOP + 6 + self.window[1])
        ImageDraw.Draw(shadow).rounded_rectangle(box, radius=RADIUS, fill=150)
        shadow = shadow.filter(ImageFilter.GaussianBlur(9)).point(lambda v: v // 24 * 24)
        image = Image.new("RGB", (w, h), self.backdrop)
        image.paste((0, 0, 0), (0, 0), shadow)
        big = Image.new("RGB", (self.window[0] * scale, self.window[1] * scale), self.bg)
        pen = ImageDraw.Draw(big)
        pen.rectangle((0, 0, big.width, BAR_H * scale - 1), fill=mix(self.bg, self.fg, 0.07))
        for i, dot in enumerate(self.meta["dots"]):
            cx = (22 + i * 22) * scale
            cy = BAR_H * scale // 2
            pen.ellipse((cx - 6 * scale, cy - 6 * scale, cx + 6 * scale, cy + 6 * scale), fill=rgb(dot))
        mask = Image.new("L", big.size, 0)
        ImageDraw.Draw(mask).rounded_rectangle((0, 0, big.width - 1, big.height - 1), radius=RADIUS * scale, fill=255)
        window = big.resize(self.window, Image.LANCZOS)
        image.paste(window, (MARGIN_X, MARGIN_TOP), mask.resize(self.window, Image.LANCZOS))
        ImageDraw.Draw(image).text(
            (w // 2, MARGIN_TOP + BAR_H // 2), "gramtty", font=self.fonts.faces[""],
            fill=mix(self.bg, self.fg, 0.6), anchor="mm",
        )
        return image

    def chip(self, text):
        if text not in self.chips:
            width = round(self.chip_font.getlength(text)) + 58
            height = 32
            scale = 3
            big = Image.new("RGBA", (width * scale, height * scale), (0, 0, 0, 0))
            pen = ImageDraw.Draw(big)
            pen.rounded_rectangle(
                (0, 0, width * scale - 1, height * scale - 1), radius=height * scale // 2,
                fill=(*mix(self.bg, self.fg, 0.14), 255),
            )
            pen.ellipse((14 * scale, 11 * scale, 24 * scale, 21 * scale), fill=(*self.accent, 255))
            pill = big.resize((width, height), Image.LANCZOS)
            ImageDraw.Draw(pill).text((36, height // 2), text, font=self.chip_font, fill=self.fg, anchor="lm")
            self.chips[text] = pill
        return self.chips[text]

    def compose(self, term, look):
        frame = self.chrome.copy()
        frame.paste(term, self.origin)
        if look["cursor"]:
            x = self.origin[0] + look["cursor"][0] * CELL_W
            y = self.origin[1] + look["cursor"][1] * CELL_H
            ImageDraw.Draw(frame).rectangle((x, y + 2, x + 1, y + CELL_H - 3), fill=self.fg)
        if look["ripple"]:
            (px, py), progress = look["ripple"]
            radius = round(5 + 15 * progress)
            ring = ripple(radius, 0.9 * (1 - progress), self.accent)
            frame.paste(ring, (round(px) - ring.width // 2, round(py) - ring.height // 2), ring)
        if look["pointer"]:
            (px, py), alpha = look["pointer"]
            sprite = fade(self.pointer, alpha)
            frame.paste(sprite, (round(px) - 3, round(py) - 3), sprite)
        if look["chip"]:
            text, alpha = look["chip"]
            pill = fade(self.chip(text), alpha)
            at = ((self.size[0] - pill.width) // 2, self.size[1] - MARGIN_BOTTOM + (MARGIN_BOTTOM - pill.height) // 2 + 3)
            frame.paste(pill, at, pill)
        return frame


def frames(steps, meta, fonts):
    cache = {}
    first = steps[0]["rows"]
    stage = Stage(meta, fonts, (len(first[0]) * CELL_W, len(first) * CELL_H))
    clock = 0
    pointer_at = None
    pointer_alpha = 0.0
    cursor_seen = (None, 0)
    chip_text, chip_alpha = "", 0.0
    last_token, last_frame = None, None

    def spot(cell):
        return (
            stage.origin[0] + cell[0] * CELL_W + CELL_W / 2,
            stage.origin[1] + cell[1] * CELL_H + CELL_H / 2,
        )

    for index, step in enumerate(steps):
        scene = Scene(step, meta, fonts, cache)
        count = max(1, round(step["ms"] / TICK))
        target = spot(step["pointer"]) if step["pointer"] else None
        origin = pointer_at if pointer_at and target else target
        cursor = step["cursor"]
        if cursor != cursor_seen[0]:
            cursor_seen = (cursor, clock)
        for i in range(count):
            local = i * TICK
            t = clock / 1000
            term = scene.image(t)
            if target:
                progress = (i + 1) / count
                glide = step.get("glide")
                amount = ease(progress) if glide == "ease" else progress if glide == "line" else 1.0
                pointer_at = (
                    origin[0] + (target[0] - origin[0]) * amount,
                    origin[1] + (target[1] - origin[1]) * amount,
                )
                pointer_alpha = min(1.0, pointer_alpha + 0.25)
            else:
                pointer_alpha = max(0.0, pointer_alpha - 0.25)
                if pointer_alpha == 0:
                    pointer_at = None
            wanted = step.get("caption", "")
            if wanted != chip_text:
                chip_alpha = max(0.0, chip_alpha - 0.25)
                if chip_alpha == 0:
                    chip_text = wanted
            else:
                chip_alpha = min(1.0, chip_alpha + 0.2)
            lit = cursor and (step.get("typing") or (clock - cursor_seen[1]) % BLINK_MS < BLINK_MS // 2)
            wave = None
            if step.get("click") and pointer_at and local < 300:
                wave = (pointer_at, local / 300)
            look = {
                "cursor": cursor if lit else None,
                "ripple": wave,
                "pointer": (pointer_at, pointer_alpha) if pointer_at and pointer_alpha > 0 else None,
                "chip": (chip_text, chip_alpha) if chip_text and chip_alpha > 0 else None,
            }
            token = (index, repr(look), int(clock / ART_TICK) if scene.moving else None)
            if token != last_token:
                last_frame = stage.compose(term, look)
                last_token = token
            yield last_frame
            clock += TICK


def load(source):
    paths = sorted(source.glob("frame-*.json"))
    if not paths:
        raise SystemExit(f"no frames in {source}")
    meta = json.loads((source / "meta.json").read_text())
    return [json.loads(path.read_text()) for path in paths], meta


def encode(stream, size, target):
    with tempfile.TemporaryDirectory() as tmp:
        raw = Path(tmp) / "frames.mkv"
        palette = Path(tmp) / "palette.png"
        rate = f"1000/{TICK}"
        writer = subprocess.Popen(
            [
                "ffmpeg", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
                "-s", f"{size[0]}x{size[1]}", "-r", rate, "-i", "-",
                "-c:v", "libx264rgb", "-qp", "0", "-preset", "ultrafast", str(raw),
            ],
            stdin=subprocess.PIPE,
        )
        count = 0
        previous, data = None, None
        for frame in stream:
            if frame is not previous:
                data = frame.tobytes()
                previous = frame
            writer.stdin.write(data)
            count += 1
        writer.stdin.close()
        if writer.wait() != 0:
            raise SystemExit("ffmpeg could not store the frames")
        subprocess.run(
            [
                "ffmpeg", "-loglevel", "error", "-y", "-i", str(raw),
                "-vf", "palettegen=stats_mode=diff:reserve_transparent=0", str(palette),
            ],
            check=True,
        )
        subprocess.run(
            [
                "ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-i", str(palette),
                "-lavfi", "paletteuse=dither=none:diff_mode=rectangle",
                "-loop", "0", str(target),
            ],
            check=True,
        )
    return count


def main():
    if len(sys.argv) != 3:
        raise SystemExit(f"usage: {sys.argv[0]} <frames dir> <out.gif>")
    source, target = Path(sys.argv[1]), Path(sys.argv[2])
    steps, meta = load(source)
    fonts = Fonts()
    stream = frames(steps, meta, fonts)
    head = next(stream)

    def chained():
        yield head
        yield from stream

    count = encode(chained(), head.size, target)
    size = target.stat().st_size / (1024 * 1024)
    print(f"{target}: {head.width}x{head.height}, {count} frames at {1000 // TICK} fps, {count * TICK / 1000:.1f} s, {size:.2f} MiB")


if __name__ == "__main__":
    main()
