# Instruments firmware

Firmware that shows the instrument UI from `../../../ui` on a Raspberry Pi Pico W with a
Waveshare Pico-ResTouch-LCD-2.8.

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
brightness is set in `src/main.rs`:

| Constant | Value | Used |
|---|---|---|
| `BRIGHTNESS_DAY` | 100 % | day |
| `BRIGHTNESS_NIGHT` | 20 % | night |

Until there is a real switch or light sensor, the demo switches to night for 5 of every 15
seconds.

## Instruments

The menu area has buttons to select the instrument (attitude indicator, altimeter, airspeed
indicator, heading indicator, vertical speed indicator or turn coordinator), see
[Touch](#touch). Until the display is touched for the first time, the demo switches the
instrument every 20 seconds. Only the selected instrument exists, so only its elements take
heap. The altimeter shows a demo altitude going up and down
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
  it is low (touched), it switches SPI1 to 2 MHz (the limit of the XPT2046), reads 8 samples of
  each axis without DMA, and switches back to the display clock. The middle half of the sorted
  samples is averaged against noise. A touch that ends while sampling is ignored.
- The touch is passed to Slint as pointer events of the left button: pressed where the touch
  starts, moved while it lasts, released where it ends. A tap on a button in the menu area is a
  click on its `TouchArea`.
- The microSD slot of the board is on SPI1 too. Its chip select (GP22) is kept high, so a card
  can't drive MISO while the touch controller is read.

### Calibration

The raw values (12 bit) are mapped linearly to the display, with the raw values at its edges in
`src/main.rs`:

| Constant | Value | Meaning |
|---|---|---|
| `TOUCH_RAW_LEFT`, `TOUCH_RAW_RIGHT` | 234, 2910 | raw x at the left and right edge |
| `TOUCH_RAW_TOP`, `TOUCH_RAW_BOTTOM` | 289, 2663 | raw y at the top and bottom edge |
| `TOUCH_SWAP_XY` | false | swap the raw axes |

The values come from the Waveshare demo code for this board in portrait and are not checked on
the device yet. If taps land in the wrong place:

1. Set `SHOW_TOUCH = true` and flash. The menu area then shows a second line with the last
   touch: `touch raw 1234, 2345 at 120, 300` (raw x, raw y, and the mapped position).
2. Touch near the four edges and note the raw values.
3. Enter them as the edge values. If raw x changes when moving up and down, set
   `TOUCH_SWAP_XY`. If an axis is mirrored, swap its two edge values (left > right is fine).

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
14.8 fps, heap peak 67 of 92 KB
```

- **fps:** frames drawn and sent per second.
- **heap peak:** highest heap use since the start, measured in the allocator.

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
block on the host (32-bit, like the RP2040) and fails if it runs out: with all three instruments
the peak is about 57 KB requested and was about 76 KB including the overhead (measured once).

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

Slint's software renderer (1.18.1) has some limitations that shape this firmware:

- **`Path` elements need a full frame buffer.** Line-by-line rendering silently skips them.
- **Partial redraws only work with the patched renderer.** The original blends paths again
  outside the redrawn region, so their edges build up. See
  `../../../issues/rp2040/partial-redraw-path` and [Vendored patches](#vendored-patches).
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

## Vendored patches

`vendor/` contains three patched crates, wired in through `[patch.crates-io]` in `Cargo.toml`.
Each change is marked with a `patched:` comment in the source.

| Crate | Why | Remove when |
|---|---|---|
| `i-slint-core` 1.18.1 | `no_std` with `software-renderer-path` doesn't compile (`f32::rem_euclid`) | Slint 1.19 is released (already fixed there) |
| `i-slint-renderer-software` 1.18.1 | See below | The changes are in a Slint release (worth offering upstream) |
| `zeno` 0.3.3 | Its rasterizer keeps about 18 KB on the stack, which overflows into the frame buffer. The patch uses heap storage with 1024 reserved cells, and adds `Scratch::with_capacity`, so the renderer can keep the cells. | Never, unless zeno or Slint change this |

Changes in the software renderer (`lib.rs` and `path.rs`):

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

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). The vendored crates keep
their own licenses, see the main [README](../../../README.md#license).
