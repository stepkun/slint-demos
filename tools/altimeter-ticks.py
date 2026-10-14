#!/usr/bin/env python3
# Copyright © 2026 Stephan Kunz
# SPDX-License-Identifier: GPL-3.0-only
"""Generates the tick paths of ui/altimeter.slint.

The ticks are static, the hands move. To keep moving hands from making the ticks be redrawn,
no box of a tick path may overlap a box of a hand at any hand angle. The ticks are therefore
grouped into paths whose boxes stay outside the reach of the hands, and the longest 100 ft hand
that never overlaps them is calculated.

Every coordinate is written as `size_float * A + B` with precomputed A and B. Each binding then
depends on a single property: written with functions of `center`, `inner_radius` and so on, the
dependency nodes of the bindings cost about 16 KB more heap (measured with tools/host-check).

Usage: python3 tools/altimeter-ticks.py > ticks.slint
Then replace the tick paths in ui/altimeter.slint with the output. The report goes to stderr.
"""

import math
import sys

# the same values as in ui/altimeter.slint and ui/theme.slint
RING_RATIO = 0.10  # Theme.ring_ratio
MARKING_WIDTH = 3.0  # Theme.marking_width in px
MAJOR_INNER = 0.85  # inner end of the long ticks, relative to the inner radius
MINOR_INNER = 0.92  # inner end of the short ticks
HAND_TIP = 0.74  # 100 ft hand, see ui/altimeter.slint
HAND_SHOULDER = 0.66
HAND_TAIL = 0.12
HAND_HALF_WIDTH = MARKING_WIDTH * 0.8
# smallest distance of a tick box from the center, relative to the inner radius
MIN_TICK_DISTANCE = 0.79

SIZE = 240  # instrument size used for the checks, the output scales with the size
TICKS = 50  # every 20 ft, a long one every 100 ft
CORNERS = [("inner", -1), ("outer", -1), ("outer", 1), ("inner", 1)]


def radius_ratio(tick, along):
    """distance from the center relative to the instrument size"""
    inner_radius = 0.5 - RING_RATIO
    if along == "outer":
        return inner_radius
    return inner_radius * (MAJOR_INNER if tick % 5 == 0 else MINOR_INNER)


def coefficients(tick, corner, axis):
    """A and B of the coordinate `size_float * A + B` of a tick corner"""
    along, side = corner
    a = math.radians(tick * 360 / TICKS)
    r = radius_ratio(tick, along)
    half_width = (MARKING_WIDTH / 2 if tick % 5 == 0 else MARKING_WIDTH / 4) * side
    if axis == "x":
        return 0.5 + r * math.sin(a), half_width * math.cos(a)
    return 0.5 - r * math.cos(a), half_width * math.sin(a)


def value(tick, corner, axis):
    a, b = coefficients(tick, corner, axis)
    return a * SIZE + b


def box(group):
    """box of a group of ticks with 1 unit margin, like the viewbox in the .slint file"""
    corners = [(t, c) for t in group for c in CORNERS]
    xs = [value(t, c, "x") for t, c in corners]
    ys = [value(t, c, "y") for t, c in corners]
    return min(xs) - 1, min(ys) - 1, max(xs) + 1, max(ys) + 1


def distance(b):
    """distance of the box from the center"""
    center = SIZE / 2
    dx = max(b[0] - center, 0, center - b[2])
    dy = max(b[1] - center, 0, center - b[3])
    return math.hypot(dx, dy)


def groups(min_distance):
    """fewest groups of neighbouring ticks whose boxes keep the distance"""
    best = None
    for start in range(TICKS):
        result, current = [], []
        for i in range(TICKS):
            tick = (start + i) % TICKS
            if current and distance(box(current + [tick])) < min_distance:
                result.append(current)
                current = [tick]
            else:
                current.append(tick)
        result.append(current)
        if best is None or len(result) < len(best):
            best = result
    return sorted(best, key=lambda g: g[0])


def hand_box(angle, inner_radius):
    s, c = math.sin(angle), math.cos(angle)
    center = SIZE / 2
    points = [
        (-HAND_TAIL, -HAND_HALF_WIDTH),
        (HAND_SHOULDER, -HAND_HALF_WIDTH),
        (HAND_TIP, 0),
        (HAND_SHOULDER, HAND_HALF_WIDTH),
        (-HAND_TAIL, HAND_HALF_WIDTH),
    ]
    xs, ys = [], []
    for along, across in points:
        along = along * inner_radius if along != 0 else 0
        xs.append(center + along * s + across * c)
        ys.append(center - along * c + across * s)
    return min(xs) - 1, min(ys) - 1, max(xs) + 1, max(ys) + 1


def overlap(a, b):
    return a[0] < b[2] and b[0] < a[2] and a[1] < b[3] and b[1] < a[3]


def expression(a, b):
    if abs(b) < 5e-5:
        return f"size_float * {a:.6f}"
    return f"size_float * {a:.6f} {'+' if b > 0 else '-'} {abs(b):.4f}"


def main():
    inner_radius = SIZE * (0.5 - RING_RATIO)
    tick_groups = groups(MIN_TICK_DISTANCE * inner_radius)
    boxes = [box(g) for g in tick_groups]
    for step in range(3600):
        hand = hand_box(math.radians(step / 10), inner_radius)
        if any(overlap(hand, b) for b in boxes):
            sys.exit(f"the 100 ft hand overlaps a tick box at {step / 10} degree")
    print(
        f"{len(tick_groups)} paths, groups {[len(g) for g in tick_groups]}, "
        f"no overlap with the 100 ft hand",
        file=sys.stderr,
    )

    out = []
    for group in tick_groups:
        corners = [(t, c) for t in group for c in CORNERS]
        x0 = min(corners, key=lambda e: value(*e, "x"))
        x1 = max(corners, key=lambda e: value(*e, "x"))
        y0 = min(corners, key=lambda e: value(*e, "y"))
        y1 = max(corners, key=lambda e: value(*e, "y"))
        ax0, bx0 = coefficients(*x0, "x")
        ax1, bx1 = coefficients(*x1, "x")
        ay0, by0 = coefficients(*y0, "y")
        ay1, by1 = coefficients(*y1, "y")
        degrees = ", ".join(f"{t * 360 / TICKS:g}deg" for t in group)
        out.append(f"    // {degrees}")
        out.append("    Path {")
        out.append(f"        viewbox-x: {expression(ax0, bx0 - 1)};")
        out.append(f"        viewbox-y: {expression(ay0, by0 - 1)};")
        out.append(f"        viewbox-width: {expression(ax1 - ax0, bx1 - bx0 + 2)};")
        out.append(f"        viewbox-height: {expression(ay1 - ay0, by1 - by0 + 2)};")
        out.append("        x: self.viewbox-x * 1px;")
        out.append("        y: self.viewbox-y * 1px;")
        out.append("        width: self.viewbox-width * 1px;")
        out.append("        height: self.viewbox-height * 1px;")
        out.append("        fill: Theme.marking;")
        for tick in group:
            for i, corner in enumerate(CORNERS):
                out.append("")
                out.append(f"        {'MoveTo' if i == 0 else 'LineTo'} {{")
                out.append(f"            x: {expression(*coefficients(tick, corner, 'x'))};")
                out.append(f"            y: {expression(*coefficients(tick, corner, 'y'))};")
                out.append("        }")
            out.append("")
            out.append("        Close { }")
        out.append("    }")
        out.append("")
    print("\n".join(out))


if __name__ == "__main__":
    main()
