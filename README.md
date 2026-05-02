# mdv

Minimal native markdown viewer. Single static binary, no webview, cold-launches in
~13 ms instead of ~200 ms.

## Build

    cargo build --release
    ./install.sh   # symlinks target/release/mdv into ~/.local/bin and installs the .desktop entry

## Use

    mdv file.md
    mdv -                   # read from stdin
    mdv --licenses          # print mdv's license + bundled fonts + all third-party deps
    mdv --trace             # print timing breakdown
    MDV_TRACE=1 mdv file.md  # same as --trace

In the app, `?` shows the full keybind list.

## License

mdv is dual-licensed under MIT or Apache-2.0; see `LICENSE-MIT` and
`LICENSE-APACHE`. Run `mdv --licenses` (or read `THIRD-PARTY-LICENSES.md`)
for the full text of every embedded dependency.

To regenerate `THIRD-PARTY-LICENSES.md` after a `cargo update` or new dep:

    cargo install cargo-about --features cli   # one-time
    cargo about generate about.hbs > THIRD-PARTY-LICENSES.md

## Bundled fonts

mdv embeds the following fonts to skip fontconfig at startup. Both are under
the SIL Open Font License 1.1. See `mdv --licenses` or the files in `assets/`
for the full text.

- Inter (Regular, Bold, Italic, BoldItalic). © 2016 The Inter Project Authors. https://github.com/rsms/inter
- JetBrains Mono (Regular, Bold, Italic). © 2020 JetBrains s.r.o. https://github.com/JetBrains/JetBrainsMono

## Stack

- `winit` 0.30 for windowing (Wayland-native on this setup)
- `softbuffer` for the surface, `tiny-skia` for 2D raster. CPU only, no GPU init cost.
- `cosmic-text` for shaping and layout, with a bundled fontdb (no system scan).
- `pulldown-cmark` for parsing, `syntect` for code highlighting.

## How it stays fast

Measured on a Ryzen 9 9800X3D, Wayland/sway, against `examples/test.md`. The
Tauri webview this replaced cold-launches in 150 to 250 ms. mdv shows the
laid-out doc in ~14 ms and adds syntax colour ~16 ms after that.

- Bundled fonts. `cosmic_text::FontSystem::new()` scans system fonts via
  fontdb (50 to 150 ms with ~10k fonts installed). Seven TTFs via
  `include_bytes!` instead: ~1 ms.
- CPU raster, not GPU. wgpu cold-init on NVIDIA Wayland costs 50 to 150 ms
  of driver setup. softbuffer + tiny-skia into wl_shm skips it.
- Defer syntect to frame 2. Frame 1 paints code blocks as plain monospace
  with the same geometry and presents at ~14 ms; frame 2 swaps in
  highlighted buffers. Doing it inline would block frame 1 for ~60 ms.
- Parallel highlight precompute, one worker per block, spawned right after
  parse. The bg threads run through the entire window-setup-to-first-paint
  path; on this hardware 3 blocks finish around the time first_present fires,
  so frame 2 finds the spans already cached. Different languages compile
  regexes independently, so 3 blocks finish in ~25 ms (the slowest one)
  instead of ~75 ms summed.
- Memoize by `(lang, code, theme)`. Resize, zoom, and theme toggle hit the
  cache (<1 ms) instead of re-running syntect (~60 ms each).
- Active theme only at startup. The other theme fills lazily on first `t`,
  a one-time ~25 ms compile that is cached after.
- Tight blend loop. `Buffer::draw` runs per-pixel; the inner blend is a
  single inline call with a fast path for opaque destinations. ~1 to 2 ms
  saved per frame.
- `MDV_TRACE=1 mdv file.md` prints per-stage timing. Every choice above
  came from reading the trace.
