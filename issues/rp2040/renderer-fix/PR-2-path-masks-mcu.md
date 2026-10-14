### Title

software renderer: rasterize path masks in bands, only where redrawn

### Description

Builds on #PR1_NUMBER (paths only blended inside the dirty region).

`render_path_with_style` allocated one coverage mask for the whole path box, at one byte per
pixel, and rasterized all of it, even if only a small part of the path was redrawn. On a
microcontroller this is often the largest single allocation. A 192×192 path needs a contiguous
36.9 KB block, which on an RP2040 (264 KB RAM, 150 KB of it the frame buffer) led to
out-of-memory panics once partial rendering was used. This PR makes path rendering cheaper in
memory and time, with no change for users:

- **Only the needed part is rasterized:** the bounding box of the clip rectangles inside the path.
  With partial rendering this skips everything outside the dirty region.
- **Bands:** that part is rasterized in horizontal bands with a mask of at most `MAX_MASK_BYTES`
  (24 KB), reusing one buffer. `zeno::Mask::offset()` moves the path into each band. This puts
  an upper limit on the memory, independent of the path size.
- **Exact bands:** the points are truncated to the rasterizer's 1/256-pixel grid first, the same
  way its own fixed-point conversion does. Moving them into a band by whole pixels is then exact,
  and the coverage is bit-identical to rasterizing the whole path at once. Partial and full
  rendering give the same pixels.
- **Strokes converted once:** strokes (including dashes) are converted into a filled outline once
  with `zeno::apply`, so the bands rasterize only that outline instead of stroking the path again
  for each band.
- **No divisions in the blend loop:** `x * coverage / 255` is replaced by
  `(v + (v >> 8) + 1) >> 8`, which gives the same result for every product of two `u8`. The loop
  now walks the mask row and the buffer line together, instead of checking each pixel's position.
  Cores without a hardware divider (Cortex-M0+) benefit most.

**Result on an RP2040** (200 MHz, 240×320 RGB565, animated attitude indicator with several
paths), measured with timing points in the renderer:

| | Before | After |
|---|---|---|
| Path blend, per frame | ~27 ms | ~13 ms |
| Largest heap allocation | 36.9 KB | 24 KB (bounded by `MAX_MASK_BYTES`) |
| Partial rendering with paths | out of memory | works, heap peak 67 of 92 KB |

**Output:** unchanged. No screenshot references needed updating, and the partial-rendering
check from #PR1_NUMBER passes for all path cases. Without the grid truncation, a few
anti-aliased edge pixels differed by one colour step between bands, and that check caught it.

**Tested:** `tests/screenshots` with the `software` and `software-embed-assets` drivers
(106 passed), the unit tests of `i-slint-renderer-software`, `clippy`, `rustfmt`, and a `no_std`
build for `thumbv6m-none-eabi` with `software-renderer-path`.

The RP2040 measurements above come from the same changes backported to 1.18.1, but without the
grid truncation, which adds only a copy of the path commands per path.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
