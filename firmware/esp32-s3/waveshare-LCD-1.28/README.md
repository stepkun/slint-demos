# Instruments firmware for the ESP32-S3-LCD-1.28

Firmware that shows the instrument UI from `../../../ui` on a Waveshare ESP32-S3-LCD-1.28: an
ESP32-S3 board with a round 1.28" display, without touch. It uses the esp-hal setup of the
UNIHIKER K10 firmware in [`../unihiker-K10`](../unihiker-K10/README.md) (toolchain, allocator,
frame transfer by DMA). The display setup is the same as on the round Pico boards, see
[`../../rp2040/waveshare-LCD-1.28-B`](../../rp2040/waveshare-LCD-1.28-B/README.md).

On the square screen the ui shows only the instrument, without the menu area. The BOOT button
selects the instrument.

## Hardware

| Part | Details |
|---|---|
| MCU | ESP32-S3R2 (512 KB SRAM, 2 MB PSRAM, not used), 16 MB flash |
| Display | 1.28" round IPS, GC9A01A, 240x240, SPI |
| IMU | QMI8658 (accelerometer and gyroscope), not used yet |
| USB | CH343P USB-UART on UART0 (GPIO43, GPIO44), for flashing and the serial output |

| Signal | GPIO |
|---|---|
| LCD DC | GPIO8 |
| LCD CS | GPIO9 |
| SPI2 CLK | GPIO10 |
| SPI2 MOSI | GPIO11 |
| LCD reset | GPIO12 |
| LCD backlight | GPIO40 (LEDC channel 0) |
| BOOT button | GPIO0 (low while pressed) |
| IMU SDA, SCL | GPIO6, GPIO7 (not used) |
| IMU interrupts | GPIO47, GPIO48 (not used) |
| Battery voltage | GPIO1 (a third of the voltage, not used) |

The pins are from the Waveshare wiki and the TFT_eSPI setup of the Waveshare examples
(`Setup207_GC9A01.h`).

## Build and flash

Requirements: the Xtensa Rust toolchain of Espressif (1.98.1.0) and `espflash`, see the K10
README for why this toolchain version.

```sh
cargo install espup espflash
espup install --targets esp32s3 --toolchain-version 1.98.1.0
. ~/export-esp.sh
```

Connect the board by USB, then:

```sh
cd firmware/esp32-s3/waveshare-LCD-1.28
cargo run --release
```

`espflash` writes the bootloader, the partition table and the firmware through the CH343
USB-UART, and then shows the serial output. If it can't connect, hold BOOT and press RESET.

If `espflash` reports `Permission denied` for the serial port (`/dev/ttyACM0`), the user may
not open the CH343 (USB ID `1a86:55d3`), which belongs to the group `dialout`. The udev rules of
probe-rs cover the USB port of Espressif chips (the K10), but not the CH343. A rule on the host
(not inside a container) gives the logged-in user access:

```sh
echo 'SUBSYSTEM=="tty", ATTRS{idVendor}=="1a86", ATTRS{idProduct}=="55d3", MODE="0660", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/70-ch343.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Its name must sort before `73-seat-late.rules`, otherwise `uaccess` has no effect. Then plug
the board in again.

## BOOT button

| Press | Effect |
|---|---|
| Short | Next instrument, stops the automatic sequence |
| Long (1 s) | Starts the automatic sequence again (the next instrument every 20 s) |

At the start, the automatic sequence is on. The action happens when the button is released.
GPIO0 is a strapping pin, but it's only read as one at reset. Pressed while the board resets,
it puts the ESP32-S3 into download mode as usual.

## Display

| Setting | Value | Where |
|---|---|---|
| SPI clock | 80 MHz | `const DISPLAY_SPI_FREQUENCY` |
| Orientation | `Rotation::Deg0` (MADCTL 0x08) | `mipidsi::Builder` |
| Colours | inverted, BGR | `mipidsi::Builder` |
| Backlight | 100 %, LEDC at 20 kHz | `const BRIGHTNESS` |

80 MHz is above the GC9A01's 66 MHz (15 ns write cycle), but the Waveshare examples run it at
that speed. A full frame takes about 6 ms. If the picture shows speckles, use 40 MHz. The
backlight stays dark until the first frame has been sent, so the uninitialized panel isn't
seen.

| Problem | Fix in `src/main.rs` |
|---|---|
| Colours inverted (sky orange instead of blue) | `ColorInversion::Inverted` → `ColorInversion::Normal` |
| Red and blue swapped | `ColorOrder::Bgr` → `ColorOrder::Rgb` |
| Picture rotated | `Rotation::Deg0` → `Deg90`, `Deg180` or `Deg270` |

## Status and crashes

There is no status line without the menu area. The firmware prints the frame rate and the
heap peak once per second on the serial port, for example `30.0 fps, heap peak 58 of 128 KB`.
`cargo run --release` shows it. Panics, including running out of heap, are printed there too
(esp-backtrace). The backlight doesn't signal them like on the Picos.

## Memory

| Part | Size |
|---|---|
| Frame buffer (240x240, RGB565) | 112.5 KB |
| Heap (TLSF) | 128 KB |
| DMA copy buffer | 8 KB |
| Stack | about 60 KB |

The instruments request at most 58 KB at this size, see `tools/host-check --display 240x240`.
The 2 MB PSRAM isn't used. The `.slint` files still have to fit the 92 KB heap of the
Pico-ResTouch-LCD-2.8 on the RP2040, see `../../../CLAUDE.md`.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../../LICENSE)). Slint and zeno keep
their own licenses.
