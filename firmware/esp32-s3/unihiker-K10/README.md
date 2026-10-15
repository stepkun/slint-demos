# Instruments firmware for the UNIHIKER K10

Firmware that shows the instrument UI from `../../../ui` on a DFRobot UNIHIKER K10 (ESP32-S3
with a 2.8" display).

It runs the same UI and demo as the Pico firmwares in `../../rp2040` and `../../rp2350`, with the
same patched Slint crates from [`../../vendor`](../../vendor/README.md), whose README describes
the renderer patches. The READMEs of the Picos describe the performance work; this one lists
what is different on the K10.

## Hardware

| Part | Details |
|---|---|
| MCU | ESP32-S3 (two Xtensa LX7 cores at 240 MHz, 512 KB SRAM, 16 MB flash, 8 MB PSRAM) |
| Display | 2.8" ILI9341, 240x320, SPI, BGR, turned by 180° and mirrored horizontally |
| Touch | none, the buttons A and B select the instrument |
| IO expander | XL9535 at I2C address 0x20, carries the backlight and the buttons |

The pins come from DFRobot's MicroPython firmware for the K10
([K10_Micropython](https://github.com/DFRobot/K10_Micropython), modules `_k10_base.py` and
`mpython.py`), the official schematic wasn't reachable when this firmware was written:

| Signal | Pin |
|---|---|
| LCD SCK | GPIO12 |
| LCD MOSI | GPIO21 |
| LCD DC | GPIO13 |
| LCD CS | GPIO14 |
| LCD reset | none (software reset) |
| I2C SDA / SCL | GPIO47 / GPIO48 |
| Backlight | expander port 0 bit 0 (on/off only) |
| Button A | expander port 1 bit 4 (low while pressed) |
| Button B | expander port 0 bit 2 (low while pressed) |

## Build and flash

Requirements: the Xtensa Rust toolchain of Espressif and `espflash`.

```sh
cargo install espup espflash
espup install --targets esp32s3 --toolchain-version 1.95.0.0
. ~/export-esp.sh
```

Use the toolchain 1.95.0.0: with 1.99.0.0, esp-hal 1.2 doesn't link ("dangerous relocation:
l32r: literal placed after use" in its boot code `__pre_init`, which runs from IRAM while the
newer compiler puts its literals into flash).

Connect the K10 by USB, then:

```sh
cd firmware/esp32-s3/unihiker-K10
cargo run --release
```

`espflash` writes the bootloader, the partition table and the firmware, and then shows the
serial output of the K10, where panics are printed. If `espflash` can't connect, put the
ESP32-S3 into its download mode as described in DFRobot's K10 documentation.

## Buttons

The K10 has no touch screen, so the touch buttons of the menu area can't be used:

| Button | Action |
|---|---|
| A | previous instrument, switches the automatic sequence off |
| B | next instrument, switches the automatic sequence off |
| A and B together | switches the automatic sequence on or off |
| A held (0.5 s) | brighter, 5 % every 0.1 s while held |
| B held (0.5 s) | dimmer, 5 % every 0.1 s while held |

Short presses act when the buttons are released, a long press doesn't change the instrument.
The brightness slider in the menu area shows the brightness (5 to 100 %).

The backlight is wired to an on/off pin of the IO expander, without PWM, so a timer interrupt
switches it on and off at 200 Hz: on at the start of each 5 ms period, off after the brightness
part of it. Each switch is one I2C write of about 70 µs (the output register of port 0 is
cached), two per period, about 3 % of the CPU; at 100 % the expander isn't written at all. The
interrupt and the button reading share the I2C bus through a critical section. The brightness
command of the ILI9341 (0x51) has no effect on the K10, its CABC output isn't connected to the
backlight.

## Differences to the Pico firmwares

| | Pico W / Pico 2 | UNIHIKER K10 |
|---|---|---|
| HAL | embassy-rp, async | esp-hal 1.2, blocking loop |
| Rust target | `thumbv6m-none-eabi` / `thumbv8m.main-none-eabihf` | `xtensa-esp32s3-none-elf` (Espressif toolchain, `build-std`) |
| Flashing | `elf2uf2-rs` / `picotool` | `espflash` |
| Clock | 200 MHz / 150 MHz | 240 MHz |
| Display | ST7789, 100 MHz / 75 MHz SPI | ILI9341, 40 MHz SPI (pins through the GPIO matrix), DMA through an 8 KB copy buffer |
| Input | touch (XPT2046) | buttons A and B on the IO expander |
| Backlight | PWM, brightness slider | on/off on the IO expander, switched at 200 Hz by a timer interrupt (long press of A or B) |
| Heap | 92 KB / 256 KB | 112 KB |
| Crash signal | backlight blinking or dark | panic message on the USB serial port |

### Memory

Of the 512 KB SRAM of the ESP32-S3, about 334 KB are left for the static data and the stack of
the application (the rest is taken by the caches and the startup code, see esp-hal's
`memory.x`):

| Part | Size |
|---|---|
| Frame buffer (240 × 320 × 2 bytes) | 150 KB |
| Heap (`HEAP_SIZE`, TLSF allocator like on the Picos) | 112 KB |
| DMA copy buffer and other statics | about 13 KB |
| Code in IRAM | about 5 KB |
| Stack | about 48 KB |

The instruments need about 61 KB of heap on the RP2040, so 112 KB leave plenty of room. The UI
is shared with the RP2040, which only has a 92 KB heap: changes to the `.slint` files must
still be checked with `tools/host-check`, which simulates the RP2040. The 8 MB PSRAM isn't used.

## Status

Runs on a K10: display, buttons A and B (instruments, AUTO, brightness) and the dimmed
backlight work. With DFRobot's 180° rotation alone the picture was mirrored horizontally; a
180° rotation with a horizontal flip is the same as a vertical flip only, which the firmware
uses (`Orientation::new().flip_vertical()` in `src/main.rs`).
