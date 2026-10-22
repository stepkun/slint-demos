# Instruments firmware

Firmware that shows the instrument UI from `../../../ui` on a Raspberry Pi Pico W with a
Waveshare Pico-ResTouch-LCD-2.8. The same firmware for the Raspberry Pi Pico 2 (RP2350) is in
[`../../rp2350/waveshare-ResTouch-LCD-2.8`](../../rp2350/waveshare-ResTouch-LCD-2.8/README.md),
the same UI on a DFRobot UNIHIKER K10 (ESP32-S3) in
[`../../esp32-s3/unihiker-K10`](../../esp32-s3/unihiker-K10/README.md).

## Hardware

| Part | Details |
|---|---|
| MCU | Raspberry Pi Pico W (RP2040, 264 KB RAM, 2 MB flash) |
| Display | Waveshare Pico-ResTouch-LCD-2.8, ST7789, 240x320, SPI |
| Orientation | Portrait, rotation 0° |

Display, touch and microSD share SPI1:

| Signal | GPIO |
|---|---|
| LCD DC | GP8 |
| LCD CS | GP9 |
| SPI CLK | GP10 |
| SPI MOSI | GP11 |
| SPI MISO | GP12 (read from the touch controller) |
| LCD backlight | GP13 (PWM slice 6, channel B) |
| LCD reset | GP15 |
| Touch CS | GP16 |
| Touch interrupt | GP17 (low while touched) |
| microSD CS | GP22 (not used, kept high) |

## Build and flash

Requirements: the `thumbv6m-none-eabi` Rust target and `elf2uf2-rs`.

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
```

Hold **BOOTSEL** on the Pico while plugging it in. It then appears as the drive `RPI-RP2`.
Then run:

```sh
cd firmware/rp2040/waveshare-ResTouch-LCD-2.8
cargo run --release
```

`elf2uf2-rs` copies the firmware onto the Pico, and the Pico restarts into it.

## Clock settings

Both clocks are set in `src/main.rs`:

| Setting | Value | Default | Where |
|---|---|---|---|
| System clock | 135 MHz | 125 MHz | `const SYSTEM_CLOCK` |
| SPI clock (display) | 67.5 MHz (half of the system clock) | – | `const DISPLAY_SPI_FREQUENCY` |

- **System clock:** 135 MHz, a little above the default of 125 MHz. The highest clock the RP2040
  is officially qualified for is 200 MHz, see [A higher system clock](#a-higher-system-clock).
- **SPI clock:** the RP2040 can only divide the system clock by even numbers, so
  `DISPLAY_SPI_FREQUENCY` is a fraction of `SYSTEM_CLOCK`: half of it, 67.5 MHz. The ST7789
  datasheet specifies 62.5 MHz for writing, so 67.5 MHz is a little outside the spec. It works
  on the board this was developed on, but other panels, longer cables or a warm board may not
  cope with it, see the next section. A full frame (240 × 320 × 16 bit) takes about 18 ms.
- **ROM routines and hardware divider:** the `intrinsics` and `rom-func-cache` features of
  `embassy-rp` replace the generic software routines for integer division and floating point
  with the RP2040's hardware divider and the optimised routines in its boot ROM. The Cortex-M0+
  has neither a division instruction nor a floating-point unit, so this speeds up rendering a lot.
- **LTO:** `[profile.release]` builds with fat LTO and one codegen unit, about 3 % faster than the
  default, with about 1.1 MB of code and a longer release build.

### A higher system clock

A higher system clock is the largest possible speed-up. The code runs from the flash through the
16 KB cache of the XIP controller, and the flash runs at half the system clock (the QSPI
interface can't divide by less than 2). The counters of the XIP controller (`CTR_ACC` and
`CTR_HIT`, read in a temporary measuring firmware) showed that about half of the time of a frame
is spent waiting for the flash: the heading indicator reads it about 2.2 million times per
frame, and its 116,000 cache misses (6 %) take around 50 ms of its 106 ms. Code that runs once
per element or per text (the dirty region, the element tree, text layout) has to be read from the
flash again and again. That is also why changes that only save instructions hardly helped, see
[Performance](#performance).

A clock up to 200 MHz, the highest officially qualified one (`ClockConfig::system_freq` raises
the core voltage to 1.15 V), can make the instruments up to 45 % faster than at 135 MHz. The
firmware stays at 135 MHz for now.

## When the display does not work at the high frequency

### Symptoms of a too fast SPI clock

- Speckles or single wrong pixels, often only now and then or after the board warms up
- Wrong or shifted colours, for example a pink or green tint
- Stripes, or the picture shifted sideways
- White, black or frozen picture right after the start, while the backlight stays on

If the backlight **blinks** or goes **dark**, the cause is something else, see
[Crash signals](#crash-signals).

### Step 1: lower the SPI clock

In `src/main.rs`:

```rust
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 4;
```

Flash again. This fixes almost every SPI problem. At 135 MHz the SPI then runs at 33.75 MHz, and a
full frame (240 × 320 × 16 bit) takes about 36 ms to transfer instead of about 18 ms.

### Step 2: use the datasheet value of 62.5 MHz

If that is too slow, get exactly 62.5 MHz, the ST7789 datasheet limit. It needs a system clock
that divides evenly into it:

- **125 MHz** (RP2040 default, officially supported):

  ```rust
  const SYSTEM_CLOCK: u32 = 125_000_000;
  // ...
  const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 2;
  ```

  The SPI runs exactly at spec, but rendering is a little slower than at 135 MHz.

- **250 MHz** (not officially qualified) with `SYSTEM_CLOCK / 4`: it gives 62.5 MHz SPI with an
  even faster CPU and flash, but the core voltage has to be set by hand, and not every chip (or
  flash) runs stable at that speed. Only use it if you know what you are doing.

### Step 3: go back to the defaults

If the display still misbehaves, the problem is probably not the clock. Set everything back to
safe values and check the other causes below:

```rust
const SYSTEM_CLOCK: u32 = 125_000_000;
// ...
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 4;
```

### Why the actual SPI frequency can differ

The SPI driver rounds the divider **up**, so the actual clock is the requested value or the next
lower one possible. At 200 MHz system clock, a request of 62.5 MHz gives **50 MHz**, and anything
from 50 MHz up to 99 MHz gives 50 MHz too. Only 100 MHz or more gives 100 MHz. The firmware
therefore sets `DISPLAY_SPI_FREQUENCY` as a fraction of `SYSTEM_CLOCK`, which is always exact.

| System clock | Possible SPI clocks |
|---|---|
| 125 MHz | 62.5, 31.25, 20.8 … MHz |
| 135 MHz | 67.5, 33.75, 22.5 … MHz |
| 200 MHz | 100, 50, 33.3 … MHz |
| 250 MHz | 125, 62.5, 41.7 … MHz |

## Other display problems

| Problem | Fix in `src/main.rs` |
|---|---|
| Colours inverted (sky orange instead of blue) | `ColorInversion::Inverted` → `ColorInversion::Normal` |
| Picture upside down | `Rotation::Deg0` → `Rotation::Deg180` |
| Landscape wanted | `Rotation::Deg90` or `Deg270`, and swap `DISPLAY_WIDTH` and `DISPLAY_HEIGHT` |
| Backlight off, nothing visible | Check GP13, it switches the backlight |

## Backlight

The backlight is dimmed by PWM on GP13 at 20 kHz, which is neither visible nor audible. The
brightness is set with the slider in the menu area, from 5 % (left) to 100 % (right); it starts
at 100 %. It doesn't go down to 0, a dark display would look like a crash (see
[Crash signals](#crash-signals)). The firmware reads the `brightness` property of the ui every
frame and sets the PWM when it changed.

## Instruments

The menu area has, below the status line, a switch (AUTO) and the brightness slider, and below
them buttons to select the instrument (attitude indicator, altimeter, airspeed indicator, heading
indicator, vertical speed indicator or turn coordinator), see [Touch](#touch). With AUTO on (at
the start), the firmware shows the next instrument every 20 seconds, counted from when the switch
was turned on. Selecting an instrument with a button turns AUTO off, so it stays. Only the
selected instrument exists, so only its elements take heap. The altimeter shows a demo altitude going up and down
between 0 and 10000 ft once a minute, the airspeed indicator a speed between 30 and 180 kt every
30 seconds, the heading indicator a steady turn, once around per minute, the vertical speed
indicator a climb and descent of up to 2200 fpm every 40 seconds (the needle stops at 2100), and
the turn coordinator turns of up to 5 degree per second every 30 seconds (beyond the standard
rate marks at 3 degree per second) with the ball swinging a little.

The card of the heading indicator turns, so it is redrawn every frame, like the inner disc of
the attitude indicator. Its labels stay upright, the software renderer can't rotate text.

## Touch

The touch controller is an XPT2046 on SPI1, shared with the display:

- Once per frame, before rendering, the firmware checks the touch interrupt (GP17). Only while
  it is low (touched), it switches SPI1 to 2 MHz (the limit of the XPT2046), reads the pressure
  and both axes without DMA, and switches back to the display clock.
- Each channel is converted 10 times in a row: the first 2 conversions are dropped while the
  voltages on the panel settle after its layers were switched, the middle half of the sorted
  other 8 is averaged against noise. (The first version alternated between x and y for every
  sample and didn't check the pressure; the corners measured with it didn't fit any mapping,
  they were up to 45 px off. With settled readings and the pressure check, five targets fit
  within 2 px.)
- A light touch gives readings pulled towards the middle of the panel: touches with a pressure
  value (Z1 + 4095 − Z2) below 350 are ignored, like common XPT2046 drivers do. A touch that ends
  while sampling is ignored too.
- The touch is passed to Slint as pointer events of the left button: pressed where the touch
  starts, moved while it lasts, released where it ends. A tap on a button in the menu area is a
  click on its `TouchArea`.
- The microSD slot of the board is on SPI1 too. Its chip select (GP22) is kept high, so a card
  can't drive MISO while the touch controller is read.

### Calibration

The raw values (12 bit) are mapped to the display with an affine mapping, so a rotated or
skewed panel is corrected too:

```
x = A * raw_x + B * raw_y + C
y = D * raw_x + E * raw_y + F
```

`TOUCH_CALIBRATION` in `src/main.rs` holds `[A, B, C, D, E, F]`. The current values are fitted to
the five targets below, touched on the device: all of them are within 2 px. The panel is
mirrored on both axes, not rotated (B and D are almost 0). To calibrate again, for example for
another board:

1. Set `SHOW_TOUCH = true` and flash. The menu area then shows a second line with the last
   touch: `touch raw 1234, 2345 z 900 at 120, 300` (raw x, raw y, pressure, mapped position).
2. Select the altimeter and touch these targets firmly, noting the raw values of each:

   | Target | Position (x, y) |
   |---|---|
   | digit 0 of the altimeter, top | 120, 12 |
   | digit 3 of the altimeter, right | 223, 153 |
   | digit 7 of the altimeter, left | 17, 153 |
   | button ATT, center | 23, 298 |
   | button TC, center | 217, 298 |

   Targets inside the display are better than the corners: they are easier to hit exactly, and
   a resistive panel is least accurate at its edges.
3. Fit the mapping and copy the printed constant into `src/main.rs`:

   ```sh
   python3 tools/touch-calibration.py 120,12=RAW_X,RAW_Y 223,153=RAW_X,RAW_Y \
       17,153=RAW_X,RAW_Y 23,298=RAW_X,RAW_Y 217,298=RAW_X,RAW_Y
   ```

   It also prints how far each target is off with the fitted mapping; more than a few pixels
   means a reading was wrong, measure that target again.

## Crash signals

There is no debug output without a debug probe. The firmware therefore shows a crash through the
backlight:

| Backlight | Meaning |
|---|---|
| Blinks about 4 times per second | Out of heap memory |
| Goes dark and stays dark | Any other panic, including a DMA transfer to the display that doesn't finish within 500 ms |

A crash can be caused by the clock settings (for example a system clock above 200 MHz without
enough core voltage), but more often it's memory. See the next section.

## Display transfer with DMA

Each frame is sent to the display by DMA, so the CPU isn't busy during the transfer:

- Slint renders into a frame buffer of `Rgb565BigEndianPixel`. Its bytes are already in the
  order the ST7789 expects, so the frame buffer is sent unchanged.
- mipidsi only initialises the display. Afterwards the firmware takes the SPI bus back, sends the
  address window and the memory write command itself, and then the pixel data in one DMA
  transfer (DMA channel 0).
- Only the changed part of the frame is redrawn (`RepaintBufferType::ReusedBuffer`), and only
  the changed rows are sent. Full rows are contiguous in the frame buffer, so they still go out
  in a single transfer.
- **The chip select (GP9) must be switched for every frame:** low while sending, high afterwards.
  With chip select permanently low, the display showed nothing at all. It also leaves the bus
  free for the touch controller, which shares SPI1.
- The main loop waits only for the rest of a 17 ms frame time (`FRAME_TIME`), so the frame rate
  is at most 60 frames per second.

## Performance

The menu area shows a status line, updated once per second:

```
9.7 fps, heap peak 58 of 92 KB
```

- **fps:** frames drawn and sent per second.
- **heap peak:** highest heap use since the start, measured in the allocator (requested bytes,
  without the TLSF overhead). It reaches about 58 KB, at the airspeed indicator, which needs the
  most.

Render and send times were shown too while the performance was tuned. They were removed to make
room in the menu area; measure them again with temporary timing points (the core cycle counter
SysTick costs only a few cycles to read, the embassy clock about a microsecond, which distorts
small sections).

At 135 MHz with the instrument moving all the time, AUTO off: heading 9.7 fps, attitude
10.3–11.1 fps (13.2–14.1 without the numbers on its pitch ladder), altimeter 33 fps. About half
of the time is spent waiting for the flash, see [A higher system clock](#a-higher-system-clock).
For the heading (about 104 ms per frame): its paths about 45 ms (rasterizing 13, blending 9,
evaluating and converting the path elements 15), its 12 labels 16 ms, the dirty region 13 ms,
the element tree 11 ms, sending 16 ms.

How the heading and the attitude got there (fps at 135 MHz):

| Step | Heading | Attitude |
|---|---|---|
| Starting point | 6.6 | 10.8–11.2 |
| Jet of the heading indicator with flat ends and bevelled corners (round ones are built from arcs in software floating point, about 30 ms per frame) | 8.4 | |
| Path blending: the mask is read a word (4 pixels) at a time, empty words are skipped | 8.9–9.0 | |
| Each band of a path gets only the subpaths that reach into it | 9.4 | |
| Opaque, fully covered path pixels written instead of blended | | 11.5–12.2 |
| Aircraft symbol of the attitude indicator with flat ends and square corners | | 12.5–13.1 |
| One sky disc instead of a ring half and an inner disc (each round `Rectangle` in the redrawn square costs about 5 ms) | | 13.1–14.2 |
| Fat LTO | 9.7 | 13.5–14.4 |
| Any roll and pitch (a full turn, loops): the ladder up to ±90° in slots that move with the pitch | | 13.2–14.1 |
| Numbers on the pitch ladder every 10°, at both ends (8 texts, laid out again every frame: 737 instead of 475 allocations per frame) | | 10.3–11.1 |

Measured, but without a gain that justified the change: avoiding divisions (the hardware
divider is fast), a square root cache for rounded rectangles, a faster loop for the pixels of
glyphs (about 1 %), smaller code (`opt-level = "s"` or `"z"`: slower, the code is less optimized
and what runs per frame still doesn't fit into the 16 KB cache).

Earlier steps, the render time of the attitude indicator, measured on the device with temporary
timing points:

| Step | Render time | fps |
|---|---|---|
| Starting point: full redraw every frame | ~111 ms | 7.9 |
| Path blending without divisions, paths without command strings | ~92 ms | 9.4 |
| Partial redraws (with the renderer fix), path masks in bands | ~87 ms | 10.3 |
| Strokes converted to outlines once, background drawn only once | ~78 ms | 11.5 |
| ROM float routines and hardware divider (`intrinsics`) | ~66 ms | 13.3 |
| `sin`/`cos` calculated once instead of inlined into every coordinate | ~60 ms | 14.5 |
| Tight boxes for the static tick paths, larger mask bands | ~58 ms | 14.8 |

## Memory and Slint limitations

The RP2040 has 264 KB of RAM, used like this:

| Part | Size |
|---|---|
| Frame buffer (240 × 320 × 2 bytes) | 150 KB |
| Heap (`HEAP_SIZE`, TLSF allocator) | 92 KB, see below |
| Stack and other statics | about 17 KB |

The heap peak in the status line counts the requested bytes. The TLSF allocator needs about 20 %
more for its block headers and alignment. `tools/host-check` runs the same allocator in a 92 KB
block on the host (32-bit, like the RP2040) and fails if it runs out. Switching between all six
instruments at random, the peak is about 58 KB requested, at the airspeed indicator; the device
allocates a little more and in a different order.

Keep room for these differences. The host check passes down to a heap of 80–81 KB, for the demo
sequence and for switching at random (`--heap-kb`, `--random`, see
`../../../tools/host-check/README.md`), 11 KB to spare (fewer `Path` elements in the instruments
saved 3–6 KB). With 4 KB to spare it once crashed at the airspeed indicator.

The stack has about 16.8 KB, what the frame buffer, the heap and the statics leave of the 264 KB.
The largest stack frames are about 10 KB: zeno's rasterizer and stroker, and the constructors of
the instruments (Slint builds a component on the stack before it moves it to the heap; the
patched `VRc::new` keeps that to about 1.8 times its size, the airspeed indicator needs 9.3 KB).
The host check can't see the stack, its host has a large one: check the frames in the
disassembly after larger changes. Switching at random also has to work: an instrument
written above the shown one in `ui/basic-panel-device.slint` was once created before the old one was
destroyed, and both together didn't fit; `basic-panel-device.slint` now removes the old one first.

### Heap fragmentation

The airspeed indicator crashed the firmware when the demo switched from it back to the attitude
indicator: out of heap, although only about 42 KB were in use. Creating and destroying the
instruments leaves small blocks spread over the heap. The renderer allocated a new mask of up
to 24 KB for every path, and zeno 16 KB of rasterizer cells, several times per frame. Once the
heap was fragmented, no free block was large enough any more.

The patched renderer therefore allocates these buffers with the first path and keeps them: an
8 KB mask and the rasterizer cells of zeno (they grow to what a band needs, 512 cells are 8 KB).
No allocation is larger than 8 KB after that. A path whose band needs more than 512 cells (a
long stroke, many subpaths in one band) makes the cells grow to a 16 KB block: keep strokes
short and split paths. Keep it that way: avoid large blocks that are allocated again and
again. The host check fails if an allocation is larger than 8 KB, or if the TLSF heap runs out.

Slint's software renderer has some limitations that shape this firmware:

- **`Path` elements need a full frame buffer.** Line-by-line rendering silently skips them.
- **Partial redraws only work with the patched renderer.** The original blends paths again
  outside the redrawn region, so their edges build up. See
  `../../../issues/rp2040/partial-redraw-path` and [Slint and zeno forks](#slint-and-zeno-forks).
- **No rotation or scaling transforms.** Rotated parts of the UI are calculated with `sin()` and
  `cos()` instead.

### Guidelines for the `.slint` files

These come from measuring on the device. They matter for everything that is redrawn often:

- **Keep `Path` boxes small.** Each path is rasterized into a mask of one byte per pixel of its
  box (in bands of at most 8 KB), and a path is redrawn whenever its box overlaps the redrawn
  region. Give static paths tight boxes that don't overlap moving parts. Use `viewbox-x` and
  `viewbox-y` to keep the coordinates the same.
- **Prefer `Rectangle` with `border-radius`** over a `Path` for circles and rings.
- **Build paths from `MoveTo` / `LineTo` / `ArcTo` elements**, not from `commands` strings.
  Formatting numbers into a string and parsing them again is slow without an FPU. `for` loops
  are not supported inside a `Path` yet (slint#754), so the elements are written out.
- **Calculate expensive values once in a property, behind a function call.** The Slint compiler
  rates `sin`, `cos` and `sqrt` as cheap, so it copies a property like `nx: sin(roll)` into
  every place that uses it. The generated code also calculates in `f64`. On the RP2040 that
  meant over 100 `f64` trig calls per frame. A property whose binding calls a function is never
  inlined: `nx: sin_of(roll)` with `pure function sin_of(a: angle) -> float { return sin(a); }`
  is calculated once.
- **Avoid `mod()` in bindings that are evaluated often.** It becomes an `f64` `rem_euclid`.
- **Every element and component instance costs heap.** Nine small `Path` components for the bank
  ticks needed about 25 KB more than four combined paths.

## Slint and zeno forks

All firmwares, `tools/host-check`, the desktop app and the web demo use Slint and zeno from
forks on GitHub (github.com/stepkun/slint, branch `patched`, Slint master 1.19.0-dev, and
github.com/stepkun/zeno, branch `main`), wired in as git dependencies through the
`[patch.crates-io]` section of their `Cargo.toml` (`slint`, `slint-build` and `zeno`; the other
Slint crates come along with `slint`). The local `Cargo.lock` of each crate pins their commit,
`cargo update -p slint -p zeno` picks up new ones; the CI has none and always builds the newest
commit of the branches. Changes to the forks affect every firmware: run the host check afterwards
(see `../../../CLAUDE.md`). To work on the forks, clone them next to `slint-demos` and point the
`[patch.crates-io]` entries to their paths. The instruments themselves come from the
`aircraft-gauges` library next to `slint-demos` (`../aircraft-gauges`, a build-dependency whose
library path `build.rs` gives to the Slint compiler).

| Crate | Why |
|---|---|
| `zeno` 0.3.3 | Its rasterizer keeps about 18 KB on the stack, which overflows into the frame buffer. The fork uses heap storage instead, see below. |
| `i-slint-renderer-software` | See below. Worth offering upstream (`../../../issues/rp2040/renderer-fix`). |

Changes in the software renderer (`lib.rs` and `path.rs`, marked with `patched:` comments):

- **Partial redraw fix:** paths are only blended into the non-overlapping parts of the redrawn
  region, like all other elements. Without it, partial redraws make path edges build up.
- **Path masks in bands:** only the part of a path inside the redrawn region is rasterized, in
  horizontal bands of at most 8 KB (`MAX_MASK_BYTES`). The original allocated one mask for the
  whole path box. This bounds memory and skips work outside the redrawn region.
- **Only the subpaths of a band:** zeno processes all commands of a path in every band; each band
  gets only the subpaths that reach into its rows (a `zeno::PathData` iterator), bands without
  any are skipped.
- **Path buffers kept:** the mask, the rasterizer cells (`zeno::Scratch`), the outline of
  strokes and the extents of the subpaths are allocated with the first path and kept in a
  `thread_local!` (`PATH_BUFFERS`) instead of for every path, see
  [Heap fragmentation](#heap-fragmentation).
- **Strokes converted to outlines once** (`zeno::apply`), so the bands only rasterize the filled
  outline instead of stroking the path again for each band.
- **Blending:** the coverage is read an aligned word (4 pixels) at a time and empty words are
  skipped; an opaque color is written where it covers a pixel completely; the coverage is scaled
  without divisions, `(v + (v >> 8) + 1) >> 8` instead of `v / 255` (the same for all products
  of two `u8`).

`helper_crates/vtable` (`VRc::new`) writes the fields of a new component one by one into its
allocation instead of building it on the stack first, which made the constructors of the
instruments need about a third less stack.

Compared with the original renderer, a few pixels at anti-aliased edges differ by one colour
step, because the path is shifted into each band. That isn't visible.

The zeno fork is still needed although the renderer no longer uses `Scratch::with_capacity`:
`Mask::render` of the published zeno contains a branch with `AdaptiveStorage`, about 18 KB of
rasterizer cells on the stack, and its stack frame is reserved even when the branch isn't taken.
With the published zeno, `Mask::render_into` needed 18.7 KB of stack, more than the 16.8 KB the
firmware has.

The fix for `no_std` with `software-renderer-path` (`f32::rem_euclid` in `i-slint-core`), which
Slint 1.18.1 needed, is already in Slint master.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). Slint and zeno keep
their own licenses.
