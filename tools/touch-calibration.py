#!/usr/bin/env python3
# Copyright © 2026 Stephan Kunz
# SPDX-License-Identifier: GPL-3.0-only
"""Calibration of the touch controller of the Pico-ResTouch-LCD-2.8 firmware.

Fits the affine mapping from raw touch values to display positions,
    x = A * raw_x + B * raw_y + C,  y = D * raw_x + E * raw_y + F,
to three or more measured points (least squares with more than three), and prints the
constant for firmware/rp2040/waveshare-ResTouch-LCD-2.8/src/main.rs.

Usage: python3 tools/touch-calibration.py X,Y=RAW_X,RAW_Y ...
  X,Y          the position on the display that was touched (pixels, portrait 240x320)
  RAW_X,RAW_Y  the raw values the firmware showed for it with SHOW_TOUCH

Example with the targets from the firmware README (made-up raw values):
  python3 tools/touch-calibration.py 120,12=1700,3500 223,153=600,2200 17,153=3300,2100 \\
      23,298=3200,900 217,298=700,800
"""

import sys


def solve3(m, v):
    """solution of the 3x3 linear system m * x = v (Cramer's rule)"""

    def det(a):
        return (
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        )

    d = det(m)
    if abs(d) < 1e-9:
        sys.exit("the points lie on a line, take points spread over the display")
    result = []
    for column in range(3):
        a = [row[:] for row in m]
        for row in range(3):
            a[row][column] = v[row]
        result.append(det(a) / d)
    return result


def fit(points, axis):
    """least squares coefficients (A, B, C) of one display axis"""
    # normal equations of [raw_x, raw_y, 1] * (A, B, C) = position
    m = [[0.0] * 3 for _ in range(3)]
    v = [0.0] * 3
    for position, (rx, ry) in points:
        row = (rx, ry, 1.0)
        for i in range(3):
            v[i] += row[i] * position[axis]
            for j in range(3):
                m[i][j] += row[i] * row[j]
    return solve3(m, v)


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    points = []
    for argument in sys.argv[1:]:
        try:
            position, raw = argument.split("=")
            x, y = (float(v) for v in position.split(","))
            rx, ry = (float(v) for v in raw.split(","))
        except ValueError:
            sys.exit(f"not X,Y=RAW_X,RAW_Y: {argument}")
        points.append(((x, y), (rx, ry)))

    a, b, c = fit(points, 0)
    d, e, f = fit(points, 1)
    print(
        f"const TOUCH_CALIBRATION: [f32; 6] = "
        f"[{a:.5f}, {b:.5f}, {c:.2f}, {d:.5f}, {e:.5f}, {f:.2f}];"
    )
    # how well the mapping fits the measured points
    for (x, y), (rx, ry) in points:
        mx, my = a * rx + b * ry + c, d * rx + e * ry + f
        error = ((mx - x) ** 2 + (my - y) ** 2) ** 0.5
        print(f"  {x:5.0f},{y:5.0f} -> {mx:6.1f},{my:6.1f}  off by {error:5.1f} px", file=sys.stderr)


if __name__ == "__main__":
    main()
