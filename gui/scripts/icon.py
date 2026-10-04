"""Draws the World's application icon: an ink ring on warm black, an agent
at work beside it. Writes a 1024-pixel source for `tauri icon`.

    python scripts/icon.py src-tauri/icon-source.png
"""

import math
import random
import sys

from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
SCALE = 4  # drawn large, then reduced, for smooth strands
BG = (31, 31, 30, 255)
INK = (245, 242, 232)
ACCENT = (217, 119, 87, 255)


def main(out: str) -> None:
    big = SIZE * SCALE
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    radius = int(big * 0.22)
    draw.rounded_rectangle((0, 0, big - 1, big - 1), radius=radius, fill=BG)

    # The ring: a dozen unsteady strands, each a closed curve a little off true.
    rng = random.Random(4)
    cx, cy, r = big * 0.47, big * 0.53, big * 0.26
    for strand in range(14):
        points = []
        start = rng.random() * math.tau
        wobble = [rng.uniform(-1, 1) for _ in range(9)]
        for step in range(361):
            angle = start + step / 360 * math.tau
            bend = sum(w * math.sin((k + 2) * angle + k) for k, w in enumerate(wobble)) * 0.006
            radius_here = r * (1 + bend + (strand - 7) * 0.0016)
            points.append((cx + math.cos(angle) * radius_here, cy + math.sin(angle) * radius_here))
        alpha = rng.randint(70, 150)
        draw.line(points, fill=(*INK, alpha), width=int(SCALE * 3.2), joint="curve")

    # An agent at work, just off the ring.
    ax, ay, ar = cx + r * 0.95, cy - r * 0.98, big * 0.045
    draw.ellipse((ax - ar, ay - ar, ax + ar, ay + ar), fill=ACCENT)

    image = image.filter(ImageFilter.GaussianBlur(SCALE * 0.35))
    image.resize((SIZE, SIZE), Image.LANCZOS).save(out)


if __name__ == "__main__":
    main(sys.argv[1])
