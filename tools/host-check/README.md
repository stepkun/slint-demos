# Host check

Renders the instrument UI from `../../ui/test-app.slint` on the host the way the RP2040 firmware
in `../../firmware/rp2040/waveshare-ResTouch-LCD-2.8` does, and checks heap use and partial
redraws. Use it after changing the `.slint` files or the vendored crates, before flashing.

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
| `--frames N` | 1250 | Number of animated frames |
| `--frame-ms MS` | 67 | Simulated time per frame (about 15 fps, like the firmware) |
| `--instrument NAME` | all | Only `attitude`, `altimeter`, `airspeed` or `heading`; by default each is shown for 20 s, like in the firmware demo, then the first one again |
| `--system-alloc` | – | Only count the allocations, with the system allocator, instead of using the TLSF heap of the firmware |
| `--png FILE` | – | Save the last partially redrawn frame as PNG |

Pass the options after `--`, for example `cargo run --release -- --png last.png`.

Exit code: `0` if all checks pass, `1` if a check fails (or the PNG can't be saved), `2` for
wrong arguments, `134` (abort) if the TLSF heap runs out.

## What it does

Slint is set up exactly like in the firmware:

- `no_std` Slint 1.18.1 with the same features (`compat-1-2`, `unsafe-single-threaded`, `libm`,
  `renderer-software`, `software-renderer-path`), resources embedded for the software renderer
- the same `[patch.crates-io]` entries, pointing to the firmware's `vendor/` directory
  (`i-slint-core`, `zeno` and the patched `i-slint-renderer-software`)
- 240x320 portrait, `Rgb565BigEndianPixel` frame buffer, `RepaintBufferType::ReusedBuffer`, so
  only the changed parts are redrawn
- the demo loop of the firmware: roll as a triangle wave of ±30° over 8 s, pitch ±10° over 5 s,
  altitude 0 to 10000 ft over 60 s, airspeed 30 to 180 kt over 30 s, heading once around in
  60 s, the instrument switched
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
host check: 240x320, 1250 frames of 67 ms, TLSF heap of the firmware
heap after ui setup:          23.6 KB
heap after last frame:        42.7 KB
heap peak:                    56.0 KB (limit 92 KB)
  attitude:                   54.1 KB
  altimeter:                  51.0 KB
  airspeed:                   56.0 KB
  heading:                    54.4 KB
largest allocation, start:     8.0 KB (path buffers of the renderer)
largest allocation, frames:    5.6 KB (limit 8 KB)
allocations, first frame:   962
allocations, later frames:  538.9 on average, 324 to 1391
redrawn rows per frame:     132.2 of 320 on average
partial vs. full redraw, at the end of each instrument (limit 1 colour step):
  attitude:                 3 pixels differ, largest difference 1 colour steps
  altimeter:                0 pixels differ, largest difference 0 colour steps
  airspeed:                 0 pixels differ, largest difference 0 colour steps
  heading:                  2 pixels differ, largest difference 1 colour steps
  attitude:                 3 pixels differ, largest difference 1 colour steps
OK
```

The check fails if

| Check | Limit | Why |
|---|---|---|
| TLSF heap runs out | 92 KB | `HEAP_SIZE` of the firmware. The heap peak shows the requested bytes, per instrument too (the frame that switches to an instrument, and so creates it, counts for it). The use including the TLSF overhead isn't shown: embedded-alloc 0.7 can only measure it by walking all blocks, and that never returns when built with Rust 1.99 for i686. Measured with Rust 1.97 it was about 76 KB with all instruments. |
| Heap peak | 92 KB | The requested bytes, mainly for `--system-alloc`. |
| Largest allocation, frames | 8 KB | Allocations after the start. A large block, allocated again and again, may not be found any more once the heap is fragmented: that crashed the firmware with the airspeed indicator. The patched renderer allocates its path buffers (mask of at most 8 KB, `MAX_MASK_BYTES`, and 512 rasterizer cells) once at the start and keeps them; the largest of those is shown separately. |
| Partial vs. full redraw | 1 colour step | After the last frame of each instrument the same state is rendered again from scratch (`RepaintBufferType::NewBuffer`) and compared per RGB565 channel. The banded path masks shift anti-aliased edges by one step, which is allowed. More means partial redraws leave artefacts, like the original 1.18.1 renderer does. |

Each failed check prints a `FAIL:` line.

With the original `i-slint-renderer-software` 1.18.1 (patch entry removed), the TLSF heap runs
out in the second frame. With `--system-alloc` it fails with a 36 KB mask block and hundreds of
pixels differing by more than one colour step between partial and full redraw.
With the renderer before its path buffers were kept, the TLSF heap runs out at the switch from
the airspeed indicator back to the attitude indicator (frame 896), as on the device.

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
