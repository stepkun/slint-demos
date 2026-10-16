# Host check

Renders the instrument UI from `../../ui/test-app.slint` on the host the way the RP2040 firmware
in `../../firmware/rp2040/waveshare-ResTouch-LCD-2.8` does, and checks heap use and partial
redraws. Use it after changing the `.slint` files or the Slint and zeno forks, before flashing.

It takes a few seconds and needs no hardware.

## Usage

The check is built for 32-bit x86 (`.cargo/config.toml`), so Slint's data structures have the
same size as on the RP2040. That needs the Rust target and a 32-bit C library once:

```sh
rustup target add i686-unknown-linux-gnu
sudo apt install gcc-multilib        # Debian/Ubuntu
```

Then:

```sh
cd tools/host-check
cargo run --release
```

Options:

| Option | Default | Meaning |
|---|---|---|
| `--frames N` | 2100 | Number of animated frames |
| `--frame-ms MS` | 67 | Simulated time per frame (about 15 fps, like the firmware) |
| `--instrument NAME` | all | Only `attitude`, `altimeter`, `airspeed`, `heading`, `vertical-speed` or `turn-coordinator`; by default each is shown for 20 s, like in the firmware demo, then the first one again |
| `--random SEED` | – | Switch to a random other instrument every 1 to 10 s instead of the demo sequence, like tapping the buttons on the device. The same seed gives the same sequence |
| `--heap-kb KB` | 92 | Size of the TLSF heap. The smallest size that still passes shows the room left for the differences to the device, see below |
| `--system-alloc` | – | Only count the allocations, with the system allocator, instead of using the TLSF heap of the firmware |
| `--png FILE` | – | Save the last partially redrawn frame as PNG |

Pass the options after `--`, for example `cargo run --release -- --png last.png`.

Exit code: `0` if all checks pass, `1` if a check fails (or the PNG can't be saved), `2` for
wrong arguments, `134` (abort) if the TLSF heap runs out.

## What it does

Slint is set up exactly like in the firmware:

- `no_std` Slint with the same features (`compat-1-2`, `unsafe-single-threaded`, `libm`,
  `renderer-software`, `software-renderer-path`), resources embedded for the software renderer
- the same `[patch.crates-io]` entries, pointing to the Slint and zeno forks next to
  `slint-demos` (`../slint`, `../zeno`) with the patched `i-slint-renderer-software`
- 240x320 portrait, `Rgb565BigEndianPixel` frame buffer, `RepaintBufferType::ReusedBuffer`, so
  only the changed parts are redrawn
- the demo loop of the firmware: roll as a triangle wave of ±30° over 8 s, pitch ±10° over 5 s,
  altitude 0 to 10000 ft over 60 s, airspeed 30 to 180 kt over 30 s, heading once around in
  60 s, vertical speed ±2200 fpm over 40 s, turn rate ±5 degree per second over 30 s and slip
  ±0.6 over 12 s, the instrument switched
  every 20 s, and the status line updated once per (simulated) second

The time Slint sees is simulated, it advances by `--frame-ms` per frame.

Memory is allocated from the allocator of the firmware: the TLSF heap of `embedded-alloc` 0.7 in
a block of 92 KB, like `HEAP_SIZE`. Its overhead per block, its alignment and the fragmentation
are therefore the same as on the device. If it runs out, the check fails at once (like the
firmware) and reports the frame and the heap use, then aborts (exit code 134). A global
allocator around it also counts the requested sizes, like the firmware does for its status line.
Counting starts before the Slint setup. The frame buffers are allocated before that, with the
system allocator, they are static in the firmware.

## Report and checks

Example output:

```
host check: 240x320, 2100 frames of 67 ms, TLSF heap of 92 KB
heap after ui setup:          26.1 KB
heap after last frame:        52.1 KB
heap peak:                    59.0 KB (limit 92 KB)
  attitude:                   43.8 KB
  altimeter:                  55.9 KB
  airspeed:                   59.0 KB
  heading:                    58.9 KB
  vertical-speed:             53.0 KB
  turn-coordinator:           37.6 KB
largest allocation, start:     8.0 KB (path buffers of the renderer)
largest allocation, frames:    5.2 KB (limit 8 KB)
allocations, first frame:   698
allocations, later frames:  443.6 on average, 99 to 1383
redrawn rows per frame:     139.3 of 320 on average
partial vs. full redraw, at the end of each instrument (limit 1 colour step):
  attitude:                 3 pixels differ, largest difference 1 colour steps
  altimeter:                0 pixels differ, largest difference 0 colour steps
  airspeed:                 0 pixels differ, largest difference 0 colour steps
  heading:                  0 pixels differ, largest difference 0 colour steps
  vertical-speed:           0 pixels differ, largest difference 0 colour steps
  turn-coordinator:         0 pixels differ, largest difference 0 colour steps
  attitude:                 3 pixels differ, largest difference 1 colour steps
  altimeter:                0 pixels differ, largest difference 0 colour steps
OK
```

The check fails if

| Check | Limit | Why |
|---|---|---|
| TLSF heap runs out | 92 KB | `HEAP_SIZE` of the firmware. The heap peak shows the requested bytes, per instrument too (the peak of each frame counts for the instrument that exists in it, `shown` of the UI; the frame that creates an instrument counts for it). The use including the TLSF overhead isn't shown: embedded-alloc 0.7 can only measure it by walking all blocks, and that never returns when built with Rust 1.99 for i686. Measured with Rust 1.97 it was about 76 KB with all instruments. |
| Heap peak | 92 KB | The requested bytes, mainly for `--system-alloc`. |
| Largest allocation, frames | 8 KB | Allocations after the start. A large block, allocated again and again, may not be found any more once the heap is fragmented: that crashed the firmware with the airspeed indicator. The patched renderer allocates its path buffers (mask of at most 8 KB, `MAX_MASK_BYTES`, and 512 rasterizer cells) once at the start and keeps them; the largest of those is shown separately. |
| Partial vs. full redraw | 1 colour step | After the last frame of each instrument the same state is rendered again from scratch (`RepaintBufferType::NewBuffer`) and compared per RGB565 channel. The banded path masks shift anti-aliased edges by one step, which is allowed. More means partial redraws leave artefacts, like the original renderer does. |

Each failed check prints a `FAIL:` line.

With the original `i-slint-renderer-software` 1.18.1 (measured before the forks), the TLSF heap runs
out in the second frame. With `--system-alloc` it fails with a 36 KB mask block and hundreds of
pixels differing by more than one colour step between partial and full redraw.
With the renderer before its path buffers were kept, the TLSF heap runs out at the switch from
the airspeed indicator back to the attitude indicator (frame 896), as on the device.

## Room left

A pass in the 92 KB heap is not enough, the device allocates in a slightly different order.
Find the smallest heap that still passes, for the demo sequence and for random switching:

```sh
B=target/i686-unknown-linux-gnu/release/host-check
for kb in $(seq 92 -1 70); do $B --heap-kb $kb > /dev/null 2>&1 || { echo "fails at $kb KB"; break; }; done
for kb in $(seq 92 -1 70); do $B --random 1 --frames 4500 --heap-kb $kb > /dev/null 2>&1 || { echo "random: fails at $kb KB"; break; }; done
```

The demo sequence passes down to 82 KB, random switching down to 83 KB, 9 KB to spare. The
airspeed indicator crashed the device with 4 KB to spare. Random switching found a crash the
demo sequence can't show: switched to an instrument written above the shown one in
`ui/test-app.slint`, the new one was created before the old one was destroyed (89 KB needed).
`test-app.slint` now removes the old one first.

## Differences to the device

- **Pointer size:** i686 has 32-bit pointers like the RP2040, so the sizes match. A 64-bit build
  (another `--target`) works too, but its heap numbers are much higher and it can't use the
  92 KB TLSF heap, so it only counts.
- **Allocation order:** the Rust runtime and the host code itself allocate a little differently
  from the firmware, so the fragmentation is similar, not identical. Keep some room.
- **Stack:** stack use isn't measured.
- **Timing:** render times on the host say nothing about the device, the status line shows them
  only so its text has the same shape.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../LICENSE)).
