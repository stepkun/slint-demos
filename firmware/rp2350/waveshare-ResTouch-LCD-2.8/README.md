# Instruments firmware for the Pico 2

Firmware that shows the instrument UI from `../../../ui` on a Raspberry Pi Pico 2 (RP2350)
with a Waveshare Pico-ResTouch-LCD-2.8.

It is the RP2040 firmware in `../../rp2040/waveshare-ResTouch-LCD-2.8` with only the chip
specific parts changed: the display, touch, backlight, menu and demo work the same, and it uses
the same Slint and zeno forks. The
[README](../../rp2040/waveshare-ResTouch-LCD-2.8/README.md) of the RP2040 firmware describes the
display, the touch calibration, the performance work and the
[forks](../../rp2040/waveshare-ResTouch-LCD-2.8/README.md#slint-and-zeno-forks); this one only
lists the differences.

## Hardware

| Part | Details |
|---|---|
| MCU | Raspberry Pi Pico 2 (RP2350A, Cortex-M33 with FPU, 520 KB RAM, 4 MB flash) |
| Display | Waveshare Pico-ResTouch-LCD-2.8, ST7789, 240x320, SPI |

The Pico 2 has the pinout of the Pico, so the display board plugs in the same way and uses the
same GPIOs (see the RP2040 README). The touch calibration is the one of the RP2040 firmware;
calibrate again if touches are off, as described there.

## Build and flash

Requirements: the `thumbv8m.main-none-eabihf` Rust target and `picotool` 2.x (`elf2uf2-rs`
doesn't know the RP2350).

```sh
rustup target add thumbv8m.main-none-eabihf
```

`picotool` comes with the Raspberry Pi Pico SDK, or as a package of many Linux distributions
(it must be version 2.0 or newer for the RP2350).

Hold **BOOTSEL** on the Pico 2 while plugging it in. It then appears as the drive `RP2350`.
Then run:

```sh
cd firmware/rp2350/waveshare-ResTouch-LCD-2.8
cargo run --release
```

`picotool` loads the firmware onto the Pico 2 and restarts it.

### Combined display

The second binary of the crate, `src/bin/combined.rs`, shows the combined display of
aircraft-gauges (attitude, vertical speed, altitude, heading and airspeed, 320x240) on the whole
display in landscape, with the same demo values and no menu:

```sh
cargo run --release --bin combined
```

A tap on the display shows or hides the frame rate and the heap peak at the top (only the touch
interrupt is used, no calibration). If the picture is upside down, set `ROTATION` to
`Rotation::Deg270`. The combined display needs about 85 KB of heap and a rectangular 4:3
display: it has no firmware for the RP2040 and the round displays.

## Differences to the RP2040 firmware

| | RP2040 (Pico W) | RP2350 (Pico 2) |
|---|---|---|
| Rust target | `thumbv6m-none-eabi` | `thumbv8m.main-none-eabihf` |
| Boot | boot2 in the first 256 bytes of the flash | `IMAGE_DEF` block of embassy-rp, found by the boot ROM (`memory.x`) |
| Flashing | `elf2uf2-rs` | `picotool` |
| System clock | 135 MHz | 150 MHz (default) |
| SPI clock (display) | 67.5 MHz (half of the system clock) | 75 MHz (half of the system clock) |
| Backlight PWM | 135 MHz / 10 / 1000 = 13.5 kHz | 150 MHz / 10 / 750 = 20 kHz |
| Float and division | ROM routines and hardware divider (`intrinsics`) | FPU and divide instructions of the Cortex-M33 |
| Heap | 92 KB | 256 KB |
| Panic handler | register addresses of the RP2040 | register addresses of the RP2350 (checked against rp-pac) |

### Clocks

150 MHz is the default and the highest officially specified system clock of the RP2350;
`ClockConfig::system_freq(150_000_000)` keeps the default configuration and core voltage.
The SPI clock can be at most half of the system clock, with even dividers only: 75 MHz or
37.5 MHz. The ST7789 datasheet specifies 62.5 MHz, the RP2040 firmware runs it at 67.5 MHz and
has run it at 100 MHz without problems. If the picture shows speckles or wrong colors, use
37.5 MHz (`DISPLAY_SPI_FREQUENCY` in `src/main.rs`).

A higher system clock would be the largest possible speed-up, but is outside the specification of
the RP2350: the code runs from the flash through a cache, whose clock follows the system clock.
On the RP2040 about half of the time of a frame is spent waiting for the flash, see
[A higher system clock](../../rp2040/waveshare-ResTouch-LCD-2.8/README.md#a-higher-system-clock).
On the RP2350 a higher clock can make the instruments up to 30 % faster.

### Memory

The RP2350 has 520 KB of RAM; the firmware uses the 512 KB of SRAM0-7:

| Part | Size |
|---|---|
| Frame buffer (240 × 320 × 2 bytes) | 150 KB |
| Heap (`HEAP_SIZE`, TLSF allocator) | 256 KB |
| Other statics | about 5 KB |
| Stack | about 100 KB |

The instruments need about 58 KB of heap (requested bytes, measured on the RP2040), so the heap has plenty of room here. The UI is shared with the RP2040,
which only has a 92 KB heap: changes to the `.slint` files must still be checked with
`tools/host-check`, which simulates the RP2040.

## Status

Runs on a Pico 2: display and touch work, with the 75 MHz SPI clock and the touch
calibration of the RP2040 firmware.
