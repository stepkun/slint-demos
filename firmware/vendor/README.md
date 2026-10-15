# Vendored patches

Three patched crates, shared by all firmwares (`../rp2040`, `../rp2350`, `../esp32-s3`) and by
`../../tools/host-check`, wired in through the `[patch.crates-io]` section of their
`Cargo.toml`. Each change is marked with a `patched:` comment in the source. Changes here affect
every firmware: run the host check afterwards (see `../../CLAUDE.md`).

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
  every path, see [Heap fragmentation](../rp2040/waveshare-ResTouch-LCD-2.8/README.md#heap-fragmentation) in the RP2040 README.
- **Strokes converted to outlines once** (`zeno::apply`), so the bands only rasterize the filled
  outline instead of stroking the path again for each band.
- **Coverage scaling without divisions:** `(v + (v >> 8) + 1) >> 8` instead of `v / 255`. It
  gives the same result for all products of two `u8`.

Compared with the original renderer, a few pixels at anti-aliased edges differ by one colour
step, because the path is shifted into each band. That isn't visible.

## License

The vendored crates keep their own licenses, see the main [README](../../README.md#license).
