# CLAUDE.md

Aircraft instruments built with Slint (1.18.1). The same `.slint` UI runs on the desktop and on
a Raspberry Pi Pico W (RP2040) with a Waveshare Pico-ResTouch-LCD-2.8 (ST7789, 240x320).
The RP2040 is the target that matters: everything must work there.

## Layout

| Path | What |
|---|---|
| `ui/` | Shared UI: `theme.slint` (global `Theme`), `attitude.slint`, `test-app.slint` (`AppWindow`) |
| `src/main.rs` | Desktop app, uses the software renderer to show what the display will show |
| `firmware/rp2040/waveshare-ResTouch-LCD-2.8/` | Embassy firmware, own crate and target dir, vendored patched crates in `vendor/` |
| `issues/rp2040/` | Host reproductions, bug report and renderer patches for Slint upstream |

The firmware `README.md` documents hardware, clocks, memory, performance and the vendored
patches in detail. Read it before changing anything that runs on the device.

## Build

```sh
cargo build                                   # desktop app (root crate)
cd firmware/rp2040/waveshare-ResTouch-LCD-2.8
cargo build --release                         # firmware, needs: rustup target add thumbv6m-none-eabi
cargo run --release                           # flash, only with a Pico in BOOTSEL mode attached
```

The firmware, `issues/rp2040/host-test` and `issues/rp2040/partial-redraw-path` are separate
crates (own `[workspace]`), build them from their own directory.

Without the hardware, the final check on the display is done by the maintainer. Say clearly
what was only built or tested on the host.

## RP2040 constraints for `.slint` code

The RP2040 has 264 KB RAM (150 KB frame buffer, 92 KB heap, of which ~67 KB are used at peak
by the attitude indicator), no FPU and no divide instruction. The Slint software renderer
(1.18.1) has no rotation or scaling transforms.

- **No transforms.** Rotated parts are calculated with `sin()`/`cos()`.
- **Keep `Path` boxes small.** Each path is rasterized into a mask of 1 byte per pixel of its
  box, and redrawn whenever its box overlaps a redrawn region. Give static paths tight boxes
  that don't overlap moving parts (use `viewbox-x`/`viewbox-y` to keep the coordinates).
- **Prefer `Rectangle` with `border-radius`** over a `Path` for circles and rings.
- **Build paths from `MoveTo`/`LineTo`/`ArcTo`/`Close` elements**, never `commands` strings.
  `for` is not allowed inside a `Path` (slint#754), so the elements are written out.
- **Wrap expensive values in a function call.** The compiler inlines cheap-rated properties
  (`sin`, `cos`, `sqrt`) into every use and computes in `f64`. `nx: sin_of(roll)` with
  `pure function sin_of(a: angle) -> float { return sin(a); }` is calculated once.
  See `ux`/`uy`/`nx`/`ny` in `ui/attitude.slint`.
- **Avoid `mod()`** in frequently evaluated bindings (becomes an `f64` `rem_euclid`).
- **Few elements and component instances.** Each costs heap; combine paths where possible
  (9 small tick components cost ~25 KB more than 4 combined paths).
- **Fonts are embedded** (`ui/fonts/NotoSans-Regular.ttf`), there are no system fonts.
- Use the colours and widths from `Theme`, don't hard-code them.

## Firmware notes

- Rendering uses a full frame buffer (line-by-line rendering skips paths) with
  `RepaintBufferType::ReusedBuffer`; only changed rows are sent by DMA.
- Partial redraws with paths only work with the vendored, patched `i-slint-renderer-software`.
  Don't remove the `[patch.crates-io]` entries.
- Chip select must be toggled for each transfer (shared SPI1 with the touch controller).
- Crash signals: backlight blinking = out of heap, dark = other panic.
- The status line in the menu area shows fps, render/send time and heap peak.

## Conventions

- License GPL-3.0-only. New source files start with:
  `// Copyright © 2026 Stephan Kunz` and `// SPDX-License-Identifier: GPL-3.0-only`
- `snake_case` property names in `.slint`, comments explain why, not what.
- Commits are made by the maintainer; in cloud sessions, work on a feature branch and push it.
