# Instruments firmware for the RP2040-LCD-1.28-B

Firmware that shows the instrument UI from `../../../ui` on a Waveshare RP2040-LCD-1.28-B: an
RP2040 board with a round 1.28" display in a metal case. It's based on the firmware for the
Pico-ResTouch-LCD-2.8 in
[`../waveshare-ResTouch-LCD-2.8`](../waveshare-ResTouch-LCD-2.8/README.md). That README
explains the parts both have in common: the DMA transfer, performance, memory, and the Slint and
zeno forks.

The board has no touch and no buttons. So it shows only the instrument, without the menu area,
and switches to the next instrument every 20 seconds, like AUTO on the other boards. The
instruments are round and fill the circle of the display. The corners of the square frame
buffer lie behind the glass.

## Hardware

| Part | Details |
|---|---|
| MCU | RP2040 (264 KB RAM, 2 MB flash), on the board |
| Display | 1.28" round IPS, GC9A01A, 240x240, SPI |
| IMU | QMI8658 (accelerometer and gyroscope), not used yet |

| Signal | GPIO |
|---|---|
| LCD DC | GP8 |
| LCD CS | GP9 |
| SPI1 CLK | GP10 |
| SPI1 MOSI (DIN) | GP11 |
| LCD reset | GP12 |
| LCD backlight | GP25 (PWM slice 4, channel B) |
| IMU SDA, SCL (I2C1) | GP6, GP7 (not used) |
| IMU interrupts | GP23, GP24 (not used) |
| Battery voltage | GP29 (ADC3, half the voltage, not used) |

The display is only written to, so the SPI runs without MISO (`Spi::new_txonly`) and needs only
one DMA channel.

## Build and flash

Requirements: the `thumbv6m-none-eabi` Rust target and `elf2uf2-rs`.

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
```

Hold **BOOT** while plugging in the USB-C cable, or hold BOOT and press RESET. The board then
appears as the drive `RPI-RP2`. Then run:

```sh
cd firmware/rp2040/waveshare-LCD-1.28-B
cargo run --release
```

## Clocks

| Setting | Value | Where |
|---|---|---|
| System clock | 125 MHz (RP2040 default) | `const SYSTEM_CLOCK` |
| SPI clock (display) | 62.5 MHz | `const DISPLAY_SPI_FREQUENCY` |

The RP2040 divides the SPI clock from the system clock by even numbers only. At 125 MHz,
62.5 MHz comes out exactly, within the GC9A01's 66 MHz limit (15 ns write cycle). A full frame
of 240x240 then takes about 15 ms to send. If the picture shows speckles, use 31.25 MHz.

A higher system clock is the largest possible speed-up: the code runs from the flash through a
16 KB cache, the flash at half the system clock, and about half of the time of a frame is spent
waiting for it (see
[A higher system clock](../waveshare-ResTouch-LCD-2.8/README.md#a-higher-system-clock)). A clock
up to 200 MHz, the highest officially qualified one, can make the instruments up to 45 % faster.
The firmware stays at the default for now.

## Display problems

| Problem | Fix in `src/main.rs` |
|---|---|
| Colours inverted (sky orange instead of blue) | `ColorInversion::Inverted` → `ColorInversion::Normal` |
| Red and blue swapped | `ColorOrder::Bgr` → `ColorOrder::Rgb` |
| Picture rotated | `Rotation::Deg0` → `Deg90`, `Deg180` or `Deg270` |
| Backlight off, nothing visible | Check GP25, it switches the backlight |

The settings (inverted, BGR) follow the Waveshare examples and were confirmed on the device.

## Backlight

The backlight is dimmed by PWM on GP25 at 12.5 kHz. Without a slider, the brightness is the
constant `BRIGHTNESS` (100 %).

## Crash signals

The same as on the other RP2040 board, through the backlight:

| Backlight | Meaning |
|---|---|
| Blinks about 4 times per second | Out of heap memory |
| Goes dark and stays dark | Any other panic, including a DMA transfer that doesn't finish within 500 ms |

## Memory

| Part | Size |
|---|---|
| Frame buffer (240x240, RGB565) | 112.5 KB |
| Heap (TLSF) | 128 KB |
| Stack and other statics | about 18.5 KB for the stack |

The smaller frame buffer leaves room for a larger heap than the 92 KB of the
Pico-ResTouch-LCD-2.8. Without the menu area the instruments also need less. With
`tools/host-check --display 240x240`, the heading indicator requests the most, 58 KB at its
peak. The demo sequence and random switching both pass down to a 77 KB heap, so 128 KB leaves
plenty of room for the differences to the device.

```sh
cd tools/host-check
cargo run --release -- --display 240x240
```

The `.slint` files still have to fit the 92 KB heap of the Pico-ResTouch-LCD-2.8, see
`../../../CLAUDE.md`.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). Slint and zeno keep
their own licenses.
