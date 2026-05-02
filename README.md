# vmd

Minimal native markdown viewer. Single static binary, no webview, cold-launches in
~15 to 30 ms depending on doc size, vs ~200 ms for the Tauri webview it replaces.

## Build

    cargo build --release
    ./install.sh   # symlinks target/release/vmd into ~/.local/bin and installs the .desktop entry

## Use

    vmd file.md
    vmd -                   # read from stdin
    vmd --licenses          # print vmd's license + bundled fonts + all third-party deps
    vmd --trace             # print timing breakdown
    VMD_TRACE=1 vmd file.md  # same as --trace

In the app, `?` shows the full keybind list.

## License

vmd is dual-licensed under MIT or Apache-2.0; see `LICENSE-MIT` and
`LICENSE-APACHE`. Run `vmd --licenses` (or read `THIRD-PARTY-LICENSES.md`)
for the full text of every embedded dependency.

To regenerate `THIRD-PARTY-LICENSES.md` after a `cargo update` or new dep:

    cargo install cargo-about --features cli   # one-time
    cargo about generate about.hbs > THIRD-PARTY-LICENSES.md

## Bundled fonts

vmd embeds the following fonts to skip fontconfig at startup. Both are under
the SIL Open Font License 1.1. See `vmd --licenses` or the files in `assets/`
for the full text.

- Inter (Regular, Bold, Italic, BoldItalic). © 2016 The Inter Project Authors. https://github.com/rsms/inter
- JetBrains Mono (Regular, Bold, Italic). © 2020 JetBrains s.r.o. https://github.com/JetBrains/JetBrainsMono

## Stack

- `winit` 0.30 for windowing (Wayland-native on this setup)
- `softbuffer` for the surface, `tiny-skia` for 2D raster. CPU only, no GPU init cost.
- `cosmic-text` 0.19 for shaping and layout, with a bundled fontdb (no system scan).
  OpenType features enabled: Inter `ss02` (disambiguates I/l/1), JetBrains Mono
  `calt`/`liga` (programming ligatures: `->`, `=>`, `!=`, ...).
- `pulldown-cmark` for parsing, `syntect` for code highlighting.

## How it stays fast

Measured on a Ryzen 9 9800X3D, Wayland/sway, against `examples/test.md`. Numbers
scale with doc size; the doc here is on the heavy side (5 code blocks, lists,
tables, footnotes, ~270 lines). A typical README cold-launches in ~15 ms.

- Bundled fonts. `cosmic_text::FontSystem::new()` scans system fonts via
  fontdb (50 to 150 ms with ~10k fonts installed). Seven TTFs via
  `include_bytes!` instead: ~1 ms.
- CPU raster, not GPU. wgpu cold-init on NVIDIA Wayland costs 50 to 150 ms
  of driver setup. softbuffer + tiny-skia into wl_shm skips it.
- Defer syntect to frame 2. Frame 1 paints code blocks as plain monospace
  with the same geometry; frame 2 swaps in highlighted buffers. Doing it
  inline would block frame 1 for ~60 ms with 3 code blocks, ~100 ms with 5.
- Parallel highlight precompute, one worker per block, spawned right after
  parse. Different languages compile regexes independently, so N blocks
  finish in roughly the time of the slowest single one rather than summed.
  Bg threads run through the entire window-setup-to-first-paint path.
- Memoize by `(lang, code, theme)`. Resize, zoom, and theme toggle hit the
  cache (<1 ms) instead of re-running syntect (~60 ms+ each).
- Active theme only at startup. The other theme fills lazily on first `t`,
  a one-time compile that is cached after.
- Tight per-glyph blend. We iterate `Buffer::layout_runs` directly and call
  `SwashCache::with_pixels` per glyph, with a fast path in the blender for
  the common opaque-destination case (skips post-blend premultiplication).
- `VMD_TRACE=1 vmd file.md` prints per-stage timing. Every choice above
  came from reading the trace.
