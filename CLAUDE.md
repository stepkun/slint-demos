# CLAUDE.md

Aircraft instruments built with Slint (master, 1.19.0-dev, from the fork github.com/stepkun/slint, branch `patched`). The same `.slint` instruments run on the desktop
(all six at once) and on a Raspberry Pi Pico W (RP2040) with a Waveshare Pico-ResTouch-LCD-2.8
(ST7789, 240x320, one instrument at a time with a menu).
The RP2040 is the target that matters: everything must work there.

## Layout

| Path | What |
|---|---|
| `ui/` | The windows: `basic-panel-device.slint` (`BasicPanelDevice` of the device: one instrument and the menu with the selector), `basic-panel.slint` (`BasicPanel` of the desktop app: all six in the standard layout), `combined.slint` (`CombinedPanel`: the combined display of aircraft-gauges, 320x240, with a status overlay). A crate includes the code of one compiled `.slint` file, so `desktop.slint` exports `BasicPanel` and `CombinedPanel` (desktop app, web demo) and `device.slint` `BasicPanelDevice` and `CombinedPanel` (RP2350 ResTouch firmware); the other firmwares and `tools/host-check` compile `basic-panel-device.slint` |
| `../aircraft-gauges` | The instruments as a Slint library crate, next to `slint-demos` (own git repository): `ui/theme.slint` (global `Theme`), `attitude.slint`, `altimeter.slint`, `airspeed.slint`, `heading.slint`, `vertical-speed.slint`, `turn-coordinator.slint`, `lib.slint` (exports all), `fonts/`, and `tools/dial-paths.py` (generates the static tick and arc paths between the `BEGIN`/`END generated` markers and checks how much of their boxes a hand box covers at any angle: nothing, or at most `max_overlap` px for the ticks in quarters of the altimeter and the vertical speed indicator; `--check` fails if a file is not up to date). Every crate here has it as a build-dependency (`path`), its `build.rs` passes `aircraft_gauges::library_paths()` to the Slint compiler and `aircraft_gauges::FONT_SIZES` as `SLINT_FONT_SIZES`; the `.slint` files import `@aircraft-gauges/lib.slint`. The CI clones it next to the repository: the branch of the same name if aircraft-gauges has one (a change in both), otherwise its default branch |
| `src/bin/basic-panel.rs` | Desktop app `basic-panel` with `BasicPanel` (`cargo run`, the `default-run`), uses the software renderer to show what the display will show |
| `src/bin/combined.rs` | The same with `CombinedPanel`: `cargo run --bin combined` |
| `src/demo.rs` | Demo values of the instruments, shared by the desktop app, the web demo and the firmwares |
| `web/` | Web demo: the desktop apps as WebAssembly (wasm-pack), `basic-panel.html` (six instruments, calls `basic_panel()`), `combined.html` (combined display, `combined()`) and `index.html` (links to both), own crate and `[workspace]`, deployed to GitHub Pages by `.github/workflows/pages.yml` |
| github.com/stepkun/slint (branch `patched`), github.com/stepkun/zeno (`main`) | Forks of Slint (master: patched software renderer and `VRc::new`) and zeno (heap storage instead of about 18 KB on the stack), used by every crate here as git dependencies in `[patch.crates-io]` (`slint`, `slint-build`, `zeno`), see [Slint and zeno forks](firmware/rp2040/waveshare-ResTouch-LCD-2.8/README.md#slint-and-zeno-forks). The local `Cargo.lock` of each crate pins their commit, `cargo update -p slint -p zeno` picks up new ones; the CI has no `Cargo.lock` (not in the repository) and always builds the newest commit of the branches. To work on them, clone them next to `slint-demos` and point the `[patch.crates-io]` entries of the crates to the paths (`../slint/api/rs/slint`, `../slint/api/rs/build`, `../zeno`, relative to each crate) |
| `firmware/common/` | Code all firmwares share (crate `firmware-common`, a path dependency of each): TLSF heap with its counters and `init_heap!`, Slint platform and window (`init_window`), rendering of the changed rows (`render`), their transfer with the MIPI DCS address commands (`display::send_rows`, by DMA with feature `rp`, blocking otherwise), `ChipSelect`, the demo values (`src/demo.rs`, set with `show!`), the instrument sequence and the status line; the crash signal of the Picos (`crash`, feature `rp`). Only embedded-hal traits, no HAL |
| `firmware/rp2040/waveshare-ResTouch-LCD-2.8/` | Embassy firmware, own crate and target dir |
| `firmware/rp2040/waveshare-LCD-1.28-B/` | The same UI on a Waveshare RP2040-LCD-1.28-B (round GC9A01, 240x240, no touch or buttons: instrument only, automatic sequence), own crate |
| `firmware/rp2350/waveshare-ResTouch-LCD-2.8/` | The same firmware for the Raspberry Pi Pico 2 (RP2350), own crate; its second binary `combined` (`src/bin/combined.rs`, `cargo run --release --bin combined`) shows `CombinedPanel` in landscape, a tap shows the status. The combined display needs about 85 KB of heap and a rectangular 4:3 display: only this firmware has it |
| `firmware/rp2350/waveshare-Touch-LCD-1.28/` | The same UI on a Waveshare RP2350-Touch-LCD-1.28 (round GC9A01, 240x240, CST816S touch): instrument only, touch gestures evaluated by the firmware, own crate |
| `firmware/esp32-s3/unihiker-K10/` | The same UI and demo on a DFRobot UNIHIKER K10 (ESP32-S3, ILI9341, buttons instead of touch), esp-hal, own crate |
| `firmware/esp32-s3/waveshare-LCD-1.28/` | The same UI on a Waveshare ESP32-S3-LCD-1.28 (round GC9A01, 240x240, no touch): instrument only, the BOOT button selects it, esp-hal like the K10, own crate |
| `tools/touch-calibration.py` | Fits the touch calibration of the firmware to measured points |
| `tools/host-check/` | Renders the UI on the host like the firmware, checks heap use and partial redraws of every instrument |
| `issues/rp2040/` | Host reproductions, bug report and renderer patches for Slint upstream |

The firmware `README.md` documents hardware, clocks, memory and performance in detail,
its section "Slint and zeno forks" the patches. Read it before changing anything that runs on the device.

## Build

The programs are named `basic-panel` (the six instruments) and `combined` (the combined display) in
all the projects: the desktop binaries, the web pages, the firmware binaries (`[[bin]]`, their
packages keep their names), and in `aircraft-gauges` and `x-plane`.

```sh
cargo build                                   # desktop apps basic-panel and combined (root crate)
cd firmware/rp2040/waveshare-ResTouch-LCD-2.8
cargo build --release                         # firmware, needs: rustup target add thumbv6m-none-eabi
cargo run --release                           # flash, only with a Pico in BOOTSEL mode attached
cd ../waveshare-LCD-1.28-B
cargo build --release                         # round RP2040-LCD-1.28-B, same target
cd ../../rp2350/waveshare-ResTouch-LCD-2.8
cargo build --release                         # Pico 2, needs: rustup target add thumbv8m.main-none-eabihf
cargo run --release                           # flash with picotool 2.x, Pico 2 in BOOTSEL mode
cargo run --release --bin combined            # the combined display instead
cd ../waveshare-Touch-LCD-1.28
cargo build --release                         # round RP2350-Touch-LCD-1.28, same target
cd ../../esp32-s3/unihiker-K10
cargo build --release                         # K10, needs: espup install --targets esp32s3 --toolchain-version 1.98.1.0
cargo run --release                           # flash with espflash, K10 on USB
cd ../waveshare-LCD-1.28
cargo build --release                         # round ESP32-S3-LCD-1.28, same toolchain
```

The RP2350 firmware is a copy of the RP2040 one with the chip specific parts changed (target,
`memory.x` with the `IMAGE_DEF` block, clocks, heap, register addresses of the panic handler),
see its README. Keep both `src/main.rs` in step when changing the display, touch or menu code.
What is the same in all firmwares belongs in `firmware/common`; the firmwares keep the HAL
setup, pins, clocks, display init and their input (touch, buttons, gestures).
The RP2040-LCD-1.28-B firmware is a smaller copy without touch and menu: a square screen makes
`ui/basic-panel-device.slint` leave out the menu area (`if root.screen_width != root.screen_height`).
Check it with `tools/host-check --display 240x240`. The RP2350-Touch-LCD-1.28 firmware uses the
same display code with the RP2350 parts of the Pico 2 firmware, and turns touch gestures into
the `instrument`, `auto_select` and `brightness` properties. The ESP32-S3-LCD-1.28 firmware is
the K10 firmware with that display and the BOOT button, and prints fps and heap peak on the
serial port.
The K10 firmware uses esp-hal instead of embassy and buttons instead of touch, but the same
allocator, Slint setup and frame transfer as the Picos, see its README.
The RP2040, with its 92 KB heap, stays the target the UI has to fit.

The firmwares, `tools/host-check`, `web` and `issues/rp2040/partial-redraw-path` are separate
crates (own `[workspace]`), build them from their own directory. The web demo:

```sh
cd web
wasm-pack build --target web --release        # needs: rustup target add wasm32-unknown-unknown
python3 -m http.server 8000                   # then open http://localhost:8000
```

After changing the `.slint` files (here or in `../aircraft-gauges`) or the Slint and zeno forks, run the host check
(see `tools/host-check/README.md`; it is built for i686, needs
`rustup target add i686-unknown-linux-gnu` and `gcc-multilib`):

```sh
python3 ../aircraft-gauges/tools/dial-paths.py  # after changing generated dial geometry
cd tools/host-check
cargo run --release                           # every instrument for 20 s, like the firmware demo
                                              # fails when the 92 KB TLSF heap of the firmware runs
                                              # out, on an allocation above 8 KB, or when partial vs.
                                              # full redraw differ by > 1 colour step
```

It runs 32-bit with the firmware's TLSF allocator, so sizes, allocator overhead (~20 %) and
fragmentation match the device. The heap peak in the status line counts requested bytes only.
A passing host check is not enough: the allocation order of the device differs, so keep room.
Find the smallest heap that passes with `--heap-kb`, for the demo sequence and for random
switching (`--random SEED --frames 4500`), see `tools/host-check/README.md`. The airspeed
indicator crashed the device with 4 KB to spare (88 KB); now both pass in 80–81 KB, 11 KB to
spare. The host check can't see the stack (its host has a large one) and doesn't measure speed:
both need the device or the disassembly, see below.

Without the hardware, the final check on the display is done by the maintainer. Say clearly
what was only built or tested on the host.

## RP2040 constraints for `.slint` code

The RP2040 has 264 KB RAM (150 KB frame buffer, 92 KB heap, of which about 58 KB are requested
at peak on the device and in the host check (the airspeed indicator needs the most), plus ~20 %
TLSF overhead, and about 16.8 KB of stack), no FPU and no divide instruction (but a fast hardware
divider). The code runs from the flash through a 16 KB cache. The Slint software renderer has no
rotation or scaling transforms.

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
  See `ux`/`uy`/`nx`/`ny` in `ui/attitude.slint` of `aircraft-gauges`.
- **Avoid `mod()`** in frequently evaluated bindings (becomes an `f64` `rem_euclid`).
- **Few elements and component instances.** Each costs heap, stack when the instrument is
  created, and time on every frame (dirty region, element tree, one rasterizing pass per redrawn
  path); combine paths where possible (9 small tick components cost ~25 KB more than 4 combined
  paths). A path may share the colour of another (the white arc of the airspeed indicator is in
  its tick paths).
- **No large blocks allocated again and again.** Creating and destroying instruments fragments
  the heap; a large allocation then fails although enough memory is free in total (this crashed
  the airspeed indicator). The patched renderer keeps its path buffers; the host check fails on
  allocations above 8 KB.
- **Few dependencies per binding for static geometry.** Every non-constant binding and each
  property it depends on costs heap. Static dial paths are generated by `tools/dial-paths.py` (in `aircraft-gauges`)
  as `size_float * A + B` (one dependency); this saved ~16 KB in the altimeter.
- **No functions for per-frame coordinates.** Inside a function body every call reads the
  properties again (and the compiler may inline `sin`/`cos` into it), and every property read
  during a binding costs a dependency node of heap. Written out in the path element, each
  property is read once per path. This took the heading indicator from 95 to 52 KB.
- **Keep per-frame blocks small.** A path whose elements change every frame (the turning card
  of the heading indicator) rebuilds its element list each frame; split it so these blocks stay
  small (~1-2 KB) and are still found in a fragmented heap.
- **Keep strokes short.** The renderer keeps the rasterizer cells of zeno, they grow to what a
  band needs (512 cells are 8 KB); a band that needs more makes them grow to a 16 KB block,
  which doesn't fit into the heap. A stroke has an edge on both sides, so it needs about twice
  the cells of a fill. The jet outline of the heading indicator needs four open lines (in two
  paths the cells grew, in one they needed 16 KB).
- **No round joins or caps on strokes that are redrawn often.** zeno builds them from arcs in
  software floating point: about 30 ms per frame for the jet of the heading indicator, 3 ms with
  flat ends and bevel joins. Use the defaults (flat ends, miter joins) or bevel joins.
- **Few round `Rectangle`s in regions redrawn every frame.** Each costs about 5 ms per frame
  there (the attitude indicator draws one sky disc instead of a ring half and an inner disc).
- **Moving Rectangles on whole pixels.** A round `Rectangle` at a fractional position draws
  anti-aliased edge pixels outside its redrawn region, which keep old values (the ball of the
  turn coordinator); `round()` its position and size.
- **Moving parts get tight boxes that follow them,** and static paths are grouped so that their
  boxes don't overlap a moving box (checked by `tools/dial-paths.py` for every hand angle). Where
  fewer, larger groups only overlap a little, they are faster: the ticks of the altimeter and the
  vertical speed indicator are in quarters, at most 235 and 551 px of their boxes are redrawn
  with the hand (`max_overlap`).
- **The stack is small.** The largest stack frames are about 10 KB (zeno, the constructors of the
  instruments: Slint builds a component on the stack before moving it to the heap, the patched
  `VRc::new` keeps that to about 1.8 times its size). Many `Path` elements in one instrument make
  its constructor frame larger; check the frames in the disassembly after larger changes.
- **Fonts are embedded** (`ui/fonts/NotoSans-Regular.ttf` of `aircraft-gauges`), there are no
  system fonts. The glyphs are pre-rendered for the sizes in `SLINT_FONT_SIZES` (set by the
  `build.rs` of all firmwares and of `tools/host-check` from `aircraft_gauges::FONT_SIZES`), the
  font sizes of `Theme` (12, 14 and 20 px); a size in between falls back to the next smaller
  one. Keep `FONT_SIZES` and `Theme` in sync, and round scaled font sizes to whole pixels.
- Use the colours, widths and font sizes from `Theme`, don't hard-code them. Widths and font
  sizes are those at `Theme.reference_size` (the display); the instruments multiply them by
  their `scale` (exactly 1 on the device), so they keep their proportions on the desktop.
- **Only one instrument exists at a time.** `ui/basic-panel-device.slint` switches through
  `shown = -1` for one frame: the `if`s are updated in the order they are written, so switched
  to an instrument above the shown one, both would exist at once and not fit into the heap.
- **Instruments are centred squares:** every part lies in one `Rectangle` of `size`, centred in
  the area the instrument is given, so the generated paths are indented one level deeper.

## Firmware notes

- Rendering uses a full frame buffer (line-by-line rendering skips paths) with
  `RepaintBufferType::ReusedBuffer`; only changed rows are sent by DMA.
- Partial redraws with paths only work with the patched `i-slint-renderer-software` of the
  Slint fork (branch `patched`). Don't remove the `[patch.crates-io]` entries. The zeno fork is needed
  too: with the published zeno, `Mask::render_into` reserves 18.7 KB of stack, more than the
  RP2040 has.
- About half of the frame time is spent waiting for the flash (XIP cache misses, measured with
  the counters of the XIP controller). Code that runs per element or per text costs much more
  than its instructions, saving instructions often doesn't show; a higher system clock is the
  largest lever (the flash runs at half of it), see "A higher system clock" in the RP2040
  README. The RP firmwares stay at their clocks for now; their release builds use fat LTO
  (about 3 % faster).
- Measure speed on the device, with temporary timing points that are removed afterwards: the core
  cycle counter (SysTick) is cheap to read, the embassy clock costs about a microsecond and
  distorts small sections. Instruction counts on the host (callgrind) don't show flash waits or
  software floating point.
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
