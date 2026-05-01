# mdv

Minimal native markdown viewer. Single static binary, no webview, cold-launches in
~13 ms instead of ~200 ms.

## Build

    cargo build --release
    ./install.sh   # symlinks target/release/mdv into ~/.local/bin and installs the .desktop entry

## Use

    mdv file.md
    mdv -                   # read from stdin
    mdv --licenses          # print bundled font licenses
    mdv --trace             # print timing breakdown
    MDV_TRACE=1 mdv file.md  # same as --trace

In the app, `?` shows the full keybind list.

## Bundled fonts

mdv embeds the following fonts to skip fontconfig at startup. Both are under the
SIL Open Font License 1.1 — see `mdv --licenses` or the files in `assets/` for
the full text.

- **Inter** (Regular / Bold / Italic / BoldItalic) — © 2016 The Inter Project
  Authors, https://github.com/rsms/inter
- **JetBrains Mono** (Regular / Bold / Italic) — © 2020 JetBrains s.r.o.,
  https://github.com/JetBrains/JetBrainsMono

## Stack

- `winit` 0.30 for windowing (Wayland-native on this setup)
- `softbuffer` for the surface, `tiny-skia` for 2D raster (CPU; no GPU init cost)
- `cosmic-text` for shaping & layout (with bundled fontdb, no system scan)
- `pulldown-cmark` for parsing, `syntect` for code highlighting (deferred to
  the second frame to keep first-paint under ~15 ms)
