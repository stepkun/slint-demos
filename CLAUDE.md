# CLAUDE.md

Aircraft instruments built with Slint (1.18.1). The same `.slint` instruments run on the desktop
(all six at once) and on a Raspberry Pi Pico W (RP2040) with a Waveshare Pico-ResTouch-LCD-2.8
(ST7789, 240x320, one instrument at a time with a menu).
The RP2040 is the target that matters: everything must work there.

## Layout

| Path | What |
|---|---|
| `ui/` | Shared UI: `theme.slint` (global `Theme`), `attitude.slint`, `altimeter.slint`, `airspeed.slint`, `heading.slint`, `vertical-speed.slint`, `turn-coordinator.slint`, `test-app.slint` (`AppWindow` of the device: one instrument and the menu with the selector), `six-pack.slint` (`SixPack` of the desktop app: all six in the standard layout) |
| `tools/dial-paths.py` | Generates the static tick and arc paths of the instruments (between the `BEGIN`/`END generated` markers) and checks they never overlap a hand box; `--check` fails if a file is not up to date |
| `src/main.rs` | Desktop app with `SixPack`, uses the software renderer to show what the display will show |
| `src/demo.rs` | Demo values of the instruments, shared by the desktop app and the web demo |
| `web/` | Web demo: the desktop app as WebAssembly (wasm-pack), own crate and `[workspace]`, deployed to GitHub Pages by `.github/workflows/pages.yml` |
| `firmware/rp2040/waveshare-ResTouch-LCD-2.8/` | Embassy firmware, own crate and target dir, vendored patched crates in `vendor/` |
| `tools/touch-calibration.py` | Fits the touch calibration of the firmware to measured points |
| `tools/host-check/` | Renders the UI on the host like the firmware, checks heap use and partial redraws of every instrument |
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

The firmware, `tools/host-check`, `web` and `issues/rp2040/partial-redraw-path` are separate
crates (own `[workspace]`), build them from their own directory. The web demo:

```sh
cd web
wasm-pack build --target web --release        # needs: rustup target add wasm32-unknown-unknown
python3 -m http.server 8000                   # then open http://localhost:8000
```

After changing the `.slint` files or the vendored crates, run the host check
(see `tools/host-check/README.md`; it is built for i686, needs
`rustup target add i686-unknown-linux-gnu` and `gcc-multilib`):

```sh
python3 tools/dial-paths.py                   # after changing generated dial geometry
cd tools/host-check
cargo run --release                           # every instrument for 20 s, like the firmware demo
                                              # fails when the 92 KB TLSF heap of the firmware runs
                                              # out, on an allocation above 8 KB, or when partial vs.
                                              # full redraw differ by > 1 colour step
```

It runs 32-bit with the firmware's TLSF allocator, so sizes, allocator overhead (~20 %) and
fragmentation match the device. The heap peak in the status line counts requested bytes only.
A passing host check is not enough: the allocation order of the device differs, so keep room.
The airspeed indicator crashed the device when the host check still passed in an 88 KB heap
(4 KB to spare); with 83 KB (9 KB to spare) and the original 81 KB it runs.

Without the hardware, the final check on the display is done by the maintainer. Say clearly
what was only built or tested on the host.

## RP2040 constraints for `.slint` code

The RP2040 has 264 KB RAM (150 KB frame buffer, 92 KB heap, of which ~60 KB are requested at
peak, by the airspeed indicator in the host check, plus ~20 % TLSF overhead), no FPU and no
divide instruction. The Slint software renderer (1.18.1) has no rotation or scaling transforms.

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
- **No large blocks allocated again and again.** Creating and destroying instruments fragments
  the heap; a large allocation then fails although enough memory is free in total (this crashed
  the airspeed indicator). The patched renderer keeps its path buffers; the host check fails on
  allocations above 8 KB.
- **Few dependencies per binding for static geometry.** Every non-constant binding and each
  property it depends on costs heap. Static dial paths are generated by `tools/dial-paths.py`
  as `size_float * A + B` (one dependency); this saved ~16 KB in the altimeter.
- **No functions for per-frame coordinates.** Inside a function body every call reads the
  properties again (and the compiler may inline `sin`/`cos` into it), and every property read
  during a binding costs a dependency node of heap. Written out in the path element, each
  property is read once per path. This took the heading indicator from 95 to 52 KB.
- **Keep per-frame blocks small.** A path whose elements change every frame (the turning card
  of the heading indicator) rebuilds its element list each frame; split it so these blocks stay
  small (~1-2 KB) and are still found in a fragmented heap.
- **Moving Rectangles on whole pixels.** A round `Rectangle` at a fractional position draws
  anti-aliased edge pixels outside its redrawn region, which keep old values (the ball of the
  turn coordinator); `round()` its position and size.
- **Moving parts get tight boxes that follow them,** and static paths are grouped so that their
  boxes never overlap a moving box (checked by `tools/dial-paths.py` for every hand angle).
- **Fonts are embedded** (`ui/fonts/NotoSans-Regular.ttf`), there are no system fonts. The
  glyphs are pre-rendered for the sizes in `SLINT_FONT_SIZES` (`build.rs` of the firmware and of
  `tools/host-check`), `Theme.font_size` and `Theme.large_font_size`; a size in between falls
  back to the next smaller one. Keep them in sync, and round scaled font sizes to whole pixels.
- Use the colours, widths and font sizes from `Theme`, don't hard-code them. Widths and font
  sizes are those at `Theme.reference_size` (the display); the instruments multiply them by
  their `scale` (exactly 1 on the device), so they keep their proportions on the desktop.
- **Instruments are centred squares:** every part lies in one `Rectangle` of `size`, centred in
  the area the instrument is given, so the generated paths are indented one level deeper.

## Firmware notes

- Rendering uses a full frame buffer (line-by-line rendering skips paths) with
  `RepaintBufferType::ReusedBuffer`; only changed rows are sent by DMA.
- Partial redraws with paths only work with the vendored, patched `i-slint-renderer-software`.
  Don't remove the `[patch.crates-io]` entries.
- Chip select must be toggled for each transfer (shared SPI1 with the touch controller).
- Touch: XPT2046 on SPI1 (CS GP16, interrupt GP17, MISO GP12), read at 2 MHz only while touched,
  passed to Slint as pointer events. Affine calibration `TOUCH_CALIBRATION` and `SHOW_TOUCH` in
  `src/main.rs`, fitted with `tools/touch-calibration.py`, see the firmware README. The microSD
  CS (GP22) must stay high.
- Crash signals: backlight blinking = out of heap, dark = other panic.
- The menu area has the status line (fps and heap peak in one line), the AUTO switch (instrument
  sequence every 20 s, done by the firmware), the backlight slider (5-100 %,
  the firmware sets the PWM from the `brightness` property) and the instrument buttons.

## Conventions

- License GPL-3.0-only. New source files start with:
  `// Copyright © 2026 Stephan Kunz` and `// SPDX-License-Identifier: GPL-3.0-only`
- `snake_case` property names in `.slint`, comments explain why, not what.
- Commits are made by the maintainer; in cloud sessions, work on a feature branch and push it.
