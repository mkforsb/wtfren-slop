#!/usr/bin/env python3
"""Prints the screen position of the 👍 button in a screenshot, for driving the harness."""
import sys
from PIL import Image

img = Image.open(sys.argv[1]).convert("RGB")
w, h = img.size
xs, ys = [], []
px = img.load()
# The emoji is the only saturated yellow left of the chat panel; the selection is much paler.
for y in range(60, h):
    for x in range(290, 1095):
        r, g, b = px[x, y]
        if r > 230 and 150 < g < 215 and b < 90:
            xs.append(x)
            ys.append(y)
if xs:
    print(sum(xs) // len(xs), sum(ys) // len(ys))
