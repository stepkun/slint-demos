# Instruments firmware for the RP2350-Touch-LCD-1.28

Firmware that shows the instrument UI from `../../../ui` on a Waveshare RP2350-Touch-LCD-1.28:
an RP2350 board with a round 1.28" touch display. The display part is the same as in the
firmware for the RP2040-LCD-1.28-B in
[`../../rp2040/waveshare-LCD-1.28-B`](../../rp2040/waveshare-LCD-1.28-B/README.md), the chip
specific parts (clocks, memory layout, flashing) as in the Pico 2 firmware in
[`../waveshare-ResTouch-LCD-2.8`](../waveshare-ResTouch-LCD-2.8/README.md). The README of the
Pico-ResTouch-LCD-2.8 firmware in
[`../../rp2040/waveshare-ResTouch-LCD-2.8`](../../rp2040/waveshare-ResTouch-LCD-2.8/README.md)
explains what all have in common: the DMA transfer, performance, memory, and the Slint and zeno
forks.

On the square screen the ui shows only the instrument, without the menu area. Instead of the
buttons, the firmware evaluates touch gestures.

## Hardware

| Part | Details |
|---|---|
| MCU | RP2350A (520 KB RAM, 16 MB flash), on the board |
| Display | 1.28" round IPS, GC9A01A, 240x240, SPI |
| Touch | CST816S, capacitive, I2C address 0x15 |
| IMU | QMI8658 (accelerometer and gyroscope), not used yet |

| Signal | GPIO |
|---|---|
| LCD DC | GP8 |
| LCD CS | GP9 |
| SPI1 CLK | GP10 |
| SPI1 MOSI (DIN) | GP11 |
| LCD reset | GP13 |
| LCD backlight | GP25 (PWM slice 4, channel B) |
| Touch and IMU SDA, SCL (I2C1) | GP6, GP7 |
| Touch interrupt | GP21 (not used, the touch is polled) |
| Touch reset | GP22 |
| IMU interrupts | GP23, GP24 (not used) |
| Battery voltage | GP29 (ADC3, half the voltage, not used) |

The pins are from `DEV_Config.h` of the Waveshare example code for this board.

## Build and flash

Requirements: the `thumbv8m.main-none-eabihf` Rust target and picotool 2.x (elf2uf2-rs doesn't
know the RP2350), see the README of the Pico 2 firmware.

```sh
rustup target add thumbv8m.main-none-eabihf
```

Hold **BOOT** while plugging in the USB-C cable, or hold BOOT and press RESET. Then run:

```sh
cd firmware/rp2350/waveshare-Touch-LCD-1.28
cargo run --release
```

## Touch gestures

| Gesture | Effect |
|---|---|
| Tap | Next instrument, stops the automatic sequence |
| Swipe left | Next instrument, stops the automatic sequence |
| Swipe right | Previous instrument, stops the automatic sequence |
| Long press (1 s) | Starts the automatic sequence again (the next instrument every 20 s) |
| Swipe up | Brighter, 20 % per swipe |
| Swipe down | Darker, 20 % per swipe, down to 5 % |

At the start, the automatic sequence is on and the brightness is 100 %.

A gesture is evaluated when the finger is lifted. A touch that moves less than 40 px
(`TOUCH_SWIPE`) is a tap, or a long press if it is held for at least 1 s (`TOUCH_LONG_PRESS`).
Otherwise the larger of the two movements decides the direction. The firmware sets the
`instrument`, `auto_select` and `brightness` properties of the ui, like the buttons and the
slider of the menu area on the other boards. The gestures don't go to Slint as pointer events:
without the menu area, there is nothing in the ui to touch.

The CST816S is polled over I2C every frame (5 bytes from register 0x02: number of fingers, x
and y). The firmware switches off its automatic sleep at the start, otherwise it wouldn't answer
until touched. Its coordinates match the display in the orientation used here (`Rotation::Deg0`,
like in the Waveshare examples), confirmed on the device. If the swipes go the wrong way, the touch is mirrored against
the display. Then swap `Left`/`Right` or `Up`/`Down` in `Touch::gesture`.

## Clocks

| Setting | Value | Where |
|---|---|---|
| System clock | 150 MHz (RP2350 default) | `const SYSTEM_CLOCK` |
| SPI clock (display) | 75 MHz | `const DISPLAY_SPI_FREQUENCY` |
| I2C clock (touch) | 400 kHz | `const TOUCH_I2C_FREQUENCY` |

At 150 MHz, the SPI clock can be 75 or 37.5 MHz. 75 MHz is a little above the GC9A01's 66 MHz
(15 ns write cycle), but the Waveshare examples for this board run it at that speed too. A full
frame takes about 12 ms. If the picture shows speckles, use 37.5 MHz.

150 MHz is the default and the highest officially specified clock of the RP2350. A higher clock
would be the largest possible speed-up, but is outside the specification: the code runs from the
flash through a cache, whose clock follows the system clock. On the RP2040 about half of the time
of a frame is spent waiting for the flash, see
[A higher system clock](../../rp2040/waveshare-ResTouch-LCD-2.8/README.md#a-higher-system-clock).
On the RP2350 a higher clock can make the instruments up to 30 % faster.

## Display problems

| Problem | Fix in `src/main.rs` |
|---|---|
| Colours inverted (sky orange instead of blue) | `ColorInversion::Inverted` → `ColorInversion::Normal` |
| Red and blue swapped | `ColorOrder::Bgr` → `ColorOrder::Rgb` |
| Picture rotated | `Rotation::Deg0` → `Deg90`, `Deg180` or `Deg270`; the touch then no longer matches, see above |
| Backlight off, nothing visible | Check GP25, it switches the backlight |

## Crash signals

The same as on the other boards, through the backlight:

| Backlight | Meaning |
|---|---|
| Blinks about 4 times per second | Out of heap memory |
| Goes dark and stays dark | Any other panic, including a DMA transfer that doesn't finish within 500 ms |

## Memory

| Part | Size |
|---|---|
| Frame buffer (240x240, RGB565) | 112.5 KB |
| Heap (TLSF) | 256 KB |
| Stack and other statics | about 141 KB for the stack |

The instruments request at most 58 KB at this size, see `tools/host-check --display 240x240`.
The heap has plenty of room. The `.slint` files still have to fit the 92 KB heap of the
Pico-ResTouch-LCD-2.8 on the RP2040, see `../../../CLAUDE.md`.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). Slint and zeno keep
their own licenses.
