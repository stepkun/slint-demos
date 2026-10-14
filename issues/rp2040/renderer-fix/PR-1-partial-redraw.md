### Title

software renderer: only blend paths inside the dirty region

### Description

With `RepaintBufferType::ReusedBuffer` and the `path` feature, `Path` elements were blended again
outside of the redrawn region. The anti-aliased edges then built up from frame to frame, so
strokes became thicker and brighter. Fixes #ISSUE_NUMBER.

**Cause:** `RenderToBuffer` limits rectangles, textures and gradients to the dirty region
(`dirty_region.intersection()` / `foreach_ranges()`). The path functions instead passed only
`clip_geometry`, the path box intersected with the item clip, to
`path::render_filled_path` / `render_stroked_path`. So the whole visible path was blended over
pixels that weren't cleared in this frame.

**Fix:**

- `RenderToBuffer::path_clip_rects()` computes the parts of the dirty region inside the clip,
  using `PhysicalRegion::iter()`, which returns non-overlapping rectangles. That way no pixel is
  blended twice.
- `render_filled_path`, `render_stroked_path` and `render_path_with_style` take these rectangles
  (`clip_rects: &[PhysicalRect]`) instead of a single clip rectangle. They blend the mask only
  inside them, and return early if there are none.
- The coverage mask is still rasterized once per path, as before.

**Test:** `assert_with_render` in `tests/screenshots/software.rs` now does the same partial check
as `assert_with_render_by_line` already does for line-by-line rendering. It renders the whole
window, fills the center with garbage, renders only the center again into the same buffer, and
compares with the reference. Line-by-line rendering skips paths, so path cases were never
covered by a partial check before. Without the fix, the new check fails for the path cases.
Cases with `SKIP_CLIPPING` are left out, as in the line-by-line check.

The new check also covers all other cases. One of them,
`basic/window-radial-gradient-background`, differs by a maximum colour difference of 1 when the
radial gradient is drawn in a clipped span. That's the same imprecision as in
`radial-gradient-explicit-center`, which already has a `BASE_THRESHOLD`, so the case gets
`BASE_THRESHOLD=2` with a comment. All other cases pass the partial check exactly.

**Tested:** `tests/screenshots` with the `software` and `software-embed-assets` drivers
(106 passed), the unit tests of `i-slint-renderer-software`, `rustfmt`, and a `no_std` build for
`thumbv6m-none-eabi` with `software-renderer-path`. Without the renderer change, the new check
fails for all four path cases (`path`, `path_dash_array`, `path_fit_preserve_stroke`,
`path_nonsquare`).

**Context:** found while using the software renderer on an RP2040 (no_std, `software-renderer-path`)
for an animated instrument display, where partial redraws are essential. Since then it has been
running there with this fix (ported from 1.18.1).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
