# Slint software renderer fixes

Two changes for Slint's software renderer, prepared as pull requests against Slint master
(`0a4e236344`, 2026-10-13). They come from the RP2040 firmware in `../../../firmware/rp2040/waveshare-ResTouch-LCD-2.8`, where the
same changes run in the Slint fork (github.com/stepkun/slint, branch `patched`).

| File | Content |
|---|---|
| `0001-software-only-blend-paths-inside-the-dirty-region.patch` | Fix for the partial-redraw bug (`../partial-redraw-path/ISSUE.md`), plus a partial-rendering check in the screenshot tests |
| `0002-software-rasterize-path-masks-in-bands-only-where-re.patch` | Path masks in bands of at most 24 KB, only where redrawn, strokes converted once, no divisions in blending |
| `PR-1-partial-redraw.md` | Title and description for the first pull request |
| `PR-2-path-masks-mcu.md` | Title and description for the second pull request, which builds on the first |

## Submitting

1. Open the issue from `../partial-redraw-path/ISSUE.md`, and note its number.
2. Fork `slint-ui/slint` on GitHub, then in a clone of the fork:

   ```sh
   git checkout -b software-renderer-path-dirty-region origin/master
   git am /path/to/0001-software-only-blend-paths-inside-the-dirty-region.patch
   git push -u origin software-renderer-path-dirty-region
   ```

3. Open the first pull request with the text from `PR-1-partial-redraw.md`. Replace
   `#ISSUE_NUMBER` with the issue number.
4. Apply the second patch on top of the first in its own branch and open the second pull request
   with `PR-2-path-masks-mcu.md`. Replace `#PR1_NUMBER` with the number of the first pull request.
   Alternatively, wait until the first one is merged.

Slint asks contributors to sign its CLA the first time. The bot comments on the pull request.

If master has moved on, `git am -3` helps with small conflicts. The tests to run are:

```sh
cargo test --manifest-path tests/screenshots/Cargo.toml --no-default-features --features software,software-embed-assets
cargo test -p i-slint-renderer-software
```
