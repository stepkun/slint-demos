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

To keep rendering smooth, the firmware runs faster than the RP2040 defaults. Both values are
set in `src/main.rs`:

| Setting | Value | Default | Where |
|---|---|---|---|
| System clock | 200 MHz | 125 MHz | `const SYSTEM_CLOCK` |
| SPI clock (display) | 100 MHz | – | `spi_config.frequency` |

- **System clock:** 200 MHz is the highest clock the RP2040 is officially qualified for.
  `ClockConfig::system_freq` raises the core voltage to 1.15 V for it. The flash also runs from
  this clock, at half of it (100 MHz).
- **SPI clock:** the RP2040 can only divide the system clock by even numbers, so at 200 MHz the
  only choices are **100 MHz**, **50 MHz**, 33 MHz and so on. The ST7789 datasheet specifies
  **62.5 MHz** for writing, so 100 MHz is outside the spec. It works on the board this was
  developed on, but other panels, longer cables or a warm board may not cope with it.
- **ROM routines and hardware divider:** the `intrinsics` and `rom-func-cache` features of
  `embassy-rp` replace the generic software routines for integer division and floating point
  with the RP2040's hardware divider and the optimised routines in its boot ROM. The Cortex-M0+
  has neither a division instruction nor a floating-point unit, so this speeds up rendering a lot.

## When the display does not work at the high frequency

### Symptoms of a too fast SPI clock

- Speckles or single wrong pixels, often only now and then or after the board warms up
- Wrong or shifted colours, for example a pink or green tint
- Stripes, or the picture shifted sideways
- White, black or frozen picture right after the start, while the backlight stays on

If the backlight **blinks** or goes **dark**, the cause is something else, see
[Crash signals](#crash-signals).

### Step 1: lower the SPI clock to 50 MHz

In `src/main.rs`:

```rust
spi_config.frequency = 50_000_000;
```

Flash again. This fixes almost every SPI problem. A full frame (240 × 320 × 16 bit) then takes about
25 ms to transfer instead of about 12 ms, and the 200 MHz CPU still renders fast.

### Step 2: use the datasheet value of 62.5 MHz

If 50 MHz is too slow, get exactly 62.5 MHz, the ST7789 datasheet limit. It needs a system
clock that divides evenly into it:

- **125 MHz** (RP2040 default, officially supported):

  ```rust
  const SYSTEM_CLOCK: u32 = 125_000_000;
  // ...
  spi_config.frequency = 62_500_000;
  ```

  The SPI runs exactly at spec, but rendering is slower than at 200 MHz.

- **250 MHz** (not officially qualified): it gives 62.5 MHz SPI with an even faster CPU, but the
  core voltage has to be set by hand, and not every chip runs stable at that speed. Only use it
  if you know what you are doing.

### Step 3: go back to the defaults

If the display still misbehaves, the problem is probably not the clock. Set everything back to
safe values and check the other causes below:

```rust
const SYSTEM_CLOCK: u32 = 125_000_000;
// ...
spi_config.frequency = 31_250_000;
```

### Why the actual SPI frequency can differ

The SPI driver rounds the divider **up**, so the actual clock is the requested value or the next
lower one possible. At 200 MHz system clock, a request of 62.5 MHz gives **50 MHz**, and anything
from 50 MHz up to 99 MHz gives 50 MHz too. Only 100 MHz or more gives 100 MHz.

| System clock | Possible SPI clocks |
|---|---|
| 125 MHz | 62.5, 31.25, 20.8 … MHz |
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
- The main loop waits only for the rest of a 20 ms frame time (`FRAME_TIME`), so the frame rate
  is at most 50 frames per second.

## Performance

The menu area shows a status line, updated once per second:

```
14.8 fps, heap peak 61 of 92 KB
```

- **fps:** frames drawn and sent per second.
- **heap peak:** highest heap use since the start, measured in the allocator (requested bytes,
  without the TLSF overhead). It reaches 61 KB when switching between the instruments at
  random; in the host check the airspeed indicator needs the most.

Render and send times were shown too while the performance was tuned (the table below). They
were removed to make room in the menu area; measure them again with temporary timing points.

With the attitude indicator moving all the time, the firmware reaches about 15 frames per
second. Most of the remaining time goes into rasterizing and blending the moving paths
(ground, horizon, pitch ladder).

How it got there, measured on the device with temporary timing points in the renderer:

| Step | Render time | fps |
|---|---|---|
| Starting point: full redraw every frame | ~111 ms | 7.9 |
| Path blending without divisions, paths without command strings | ~92 ms | 9.4 |
| Partial redraws (with the renderer fix), path masks in bands | ~87 ms | 10.3 |
| Strokes converted to outlines once, background drawn only once | ~78 ms | 11.5 |
| ROM float routines and hardware divider (`intrinsics`) | ~66 ms | 13.3 |
| `sin`/`cos` calculated once instead of inlined into every coordinate | ~60 ms | 14.5 |
| Tight boxes for the static tick paths, larger mask bands | ~58 ms | 14.8 |

The mask bands are now at most 8 KB again instead of 24 KB, and their buffers are kept instead of
being allocated for every path, see [Heap fragmentation](#heap-fragmentation). The larger bands
had saved about 2 ms. The render time with the 8 KB bands has not been measured on the device
yet.

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
instruments at random, the peak is 61 KB requested on the device and 59 KB in the host check
(`--random`); the device allocates a little more and in a different order. Including the overhead it was about 76 KB in an older
measurement, with a peak of 57 KB requested.

Keep room for these differences. The host check passes down to a heap of 83 KB, for the demo
sequence and for switching at random (`--heap-kb`, `--random`, see
`../../../tools/host-check/README.md`), 9 KB to spare, and the device runs. With 4 KB to spare
it crashed at the airspeed indicator. Switching at random also has to work: an instrument
written above the shown one in `ui/test-app.slint` was once created before the old one was
destroyed, and both together didn't fit; `test-app.slint` now removes the old one first.

### Heap fragmentation

The airspeed indicator crashed the firmware when the demo switched from it back to the attitude
indicator: out of heap, although only about 42 KB were in use. Creating and destroying the
instruments leaves small blocks spread over the heap. The renderer allocated a new mask of up
to 24 KB for every path, and zeno 16 KB of rasterizer cells, several times per frame. Once the
heap was fragmented, no free block was large enough any more.

The patched renderer therefore allocates these buffers once, when the window is created, and
keeps them: an 8 KB mask and 512 rasterizer cells (8 KB). No allocation is larger than 8 KB
after that. Keep it that way: avoid large blocks that are allocated again and again. The host
check fails if an allocation is larger than 8 KB, or if the TLSF heap runs out.

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
local forks next to `slint-demos` (`../slint`, Slint master 1.19.0-dev, and `../zeno`), wired in
through the `[patch.crates-io]` section of their `Cargo.toml` (`slint`, `slint-build` and
`zeno`; the other Slint crates come along with `slint`). Changes there affect every firmware:
run the host check afterwards (see `../../../CLAUDE.md`).

| Crate | Why |
|---|---|
| `zeno` 0.3.3 | Its rasterizer keeps about 18 KB on the stack, which overflows into the frame buffer. The fork uses heap storage with 1024 reserved cells, and adds `Scratch::with_capacity`, so the renderer can keep the cells. |
| `i-slint-renderer-software` | See below. Worth offering upstream (`../../../issues/rp2040/renderer-fix`). |

Changes in the software renderer (`lib.rs` and `path.rs`, marked with `patched:` comments):

- **Partial redraw fix:** paths are only blended into the non-overlapping parts of the redrawn
  region, like all other elements. Without it, partial redraws make path edges build up.
- **Path masks in bands:** only the part of a path inside the redrawn region is rasterized, in
  horizontal bands of at most 8 KB (`MAX_MASK_BYTES`). The original allocated one mask for the
  whole path box. This bounds memory and skips work outside the redrawn region.
- **Path buffers kept:** the mask, the rasterizer cells (`zeno::Scratch`, 512 cells reserved)
  and the outline of strokes are allocated once with the renderer (`PathBuffers`) instead of for
  every path, see [Heap fragmentation](#heap-fragmentation).
- **Strokes converted to outlines once** (`zeno::apply`), so the bands only rasterize the filled
  outline instead of stroking the path again for each band.
- **Coverage scaling without divisions:** `(v + (v >> 8) + 1) >> 8` instead of `v / 255`. It
  gives the same result for all products of two `u8`.

Compared with the original renderer, a few pixels at anti-aliased edges differ by one colour
step, because the path is shifted into each band. That isn't visible.

The fix for `no_std` with `software-renderer-path` (`f32::rem_euclid` in `i-slint-core`), which
Slint 1.18.1 needed, is already in Slint master.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). Slint and zeno keep
their own licenses.
