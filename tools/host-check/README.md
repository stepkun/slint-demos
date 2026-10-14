# Host check

Renders the instrument UI from `../../ui/test-app.slint` on the host the way the RP2040 firmware
in `../../firmware/rp2040/waveshare-ResTouch-LCD-2.8` does, and checks heap use and partial
redraws. Use it after changing the `.slint` files or the vendored crates, before flashing.

It takes less than a second and needs no hardware.

## Usage

```sh
cd tools/host-check
cargo run --release
```

Options:

| Option | Default | Meaning |
|---|---|---|
| `--frames N` | 600 | Number of animated frames |
| `--frame-ms MS` | 67 | Simulated time per frame (about 15 fps, like the firmware) |
| `--png FILE` | – | Save the last partially redrawn frame as PNG |

Pass the options after `--`, for example `cargo run --release -- --png last.png`.

Exit code: `0` if all checks pass, `1` if a check fails (or the PNG can't be saved), `2` for
wrong arguments.

## What it does

Slint is set up exactly like in the firmware:

- `no_std` Slint 1.18.1 with the same features (`compat-1-2`, `unsafe-single-threaded`, `libm`,
  `renderer-software`, `software-renderer-path`), resources embedded for the software renderer
- the same `[patch.crates-io]` entries, pointing to the firmware's `vendor/` directory
  (`i-slint-core`, `zeno` and the patched `i-slint-renderer-software`)
- 240x320 portrait, `Rgb565BigEndianPixel` frame buffer, `RepaintBufferType::ReusedBuffer`, so
  only the changed parts are redrawn
- the demo loop of the firmware: roll as a triangle wave of ±30° over 8 s, pitch ±10° over 5 s,
  and the status line updated once per (simulated) second

The time Slint sees is simulated, it advances by `--frame-ms` per frame.

A global allocator counts the requested sizes like the allocator of the firmware. Counting
starts before the Slint setup. The frame buffers are allocated before that, they are static in
the firmware.

## Report and checks

Example output:

```
host check: 240x320, 600 frames of 67 ms
heap after ui setup:          12.0 KB
heap after last frame:        42.2 KB
heap peak:                    85.5 KB (limit 92 KB)
largest single allocation:    24.0 KB (limit 24 KB)
allocations, first frame:   1005
allocations, later frames:  817.9 on average, 817 to 840
redrawn rows per frame:     192.4 of 320 on average
partial vs. full redraw:    3 pixels differ, largest difference 1 colour steps (limit 1)
OK
```

The check fails if

| Check | Limit | Why |
|---|---|---|
| Heap peak | 92 KB | `HEAP_SIZE` of the firmware |
| Largest single allocation | 24 KB | Path masks are rasterized in bands of at most 24 KB (`MAX_MASK_BYTES` in the patched renderer). A larger block means a path mask isn't banded, and TLSF may not find a contiguous block that large. |
| Partial vs. full redraw | 1 colour step | After the last frame the same state is rendered again from scratch (`RepaintBufferType::NewBuffer`) and compared per RGB565 channel. The banded path masks shift anti-aliased edges by one step, which is allowed. More means partial redraws leave artefacts, like the original 1.18.1 renderer does. |

Each failed check prints a `FAIL:` line.

With the original `i-slint-renderer-software` 1.18.1 (patch entry removed) all three checks
fail: heap peak about 96 KB, a 36 KB mask block and several hundred pixels differing by up to
35 steps.

## Differences to the device

- **Pointer size:** the host has 64-bit pointers, the RP2040 32-bit ones. Slint's data
  structures are therefore larger on the host, so the heap numbers are higher than on the device
  (85 KB here, 67 KB shown on the device). The check is on the safe side. Path masks and other
  pixel buffers have the same size on both.
- **Fragmentation:** only the requested sizes are counted, not the overhead and fragmentation of
  the TLSF allocator. Leave some room below the limit.
- **Stack:** stack use isn't measured.
- **Timing:** render times on the host say nothing about the device, the status line shows them
  only so its text has the same shape.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../../LICENSE)).
