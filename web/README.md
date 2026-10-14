# Web demo

The desktop app with all six instruments (`../ui/six-pack.slint`) as WebAssembly in the browser,
animated with the same demo values (`../src/demo.rs`). It uses the web backend of Slint: winit
draws into the canvas of `index.html`.

The workflow `../.github/workflows/pages.yml` builds it for every push and pull request and
deploys `main` to GitHub Pages.

## Build and run locally

Needs the WebAssembly target and [wasm-pack](https://github.com/drager/wasm-pack) once:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

Then:

```sh
cd web
wasm-pack build --target web --release    # writes the module to pkg/
python3 -m http.server 8000               # any static web server, then open http://localhost:8000
```

The page has to be served over HTTP, the browser doesn't load WebAssembly modules from `file://`.

`cargo build` and `cargo clippy` in this directory build for `wasm32-unknown-unknown`
(`.cargo/config.toml`), the CI checks it like the other crates.

## Notes

- `std::time::Instant` panics on `wasm32-unknown-unknown`, the demo clock comes from `web-time`.
- The event loop of the web backend ends the start function with an exception on purpose ("Using
  exceptions for control flow"), the instruments keep running in the browser. `index.html` treats
  that as a successful start.
- Unlike the desktop app (software renderer, like the device), the browser uses the default
  renderer of the web backend (WebGL), so anti-aliasing can look slightly different.

## GitHub Pages

The deployment needs GitHub Pages switched on once: repository Settings → Pages → Source
"GitHub Actions". The page is then at `https://<owner>.github.io/<repository>/`.

## License

GPL-3.0-only, like the whole project (see [LICENSE](../LICENSE)).
