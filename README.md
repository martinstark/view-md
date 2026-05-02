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
- `pulldown-cmark` for parsing, `syntect` for code highlighting

## How it stays fast

All numbers measured on a Ryzen 9 9800X3D, Wayland (sway), against
`examples/test.md` (lists, table, footnotes, three code blocks). The Tauri
webview this replaced cold-launches in **150–250 ms**. mdv reaches first paint
in **~14 ms** and fully syntax-coloured in **~30 ms**. The choices that get it
there, biggest gain first:

- **Bundle fonts in the binary, skip fontconfig.** `cosmic_text::FontSystem::new()`
  defaults to a system font scan via fontdb — about **50–150 ms** on a typical
  Linux box (~10k fonts in `fc-list`). Loading 7 TTFs from `include_bytes!`
  takes **~1 ms**. *Saves ~50–150 ms.*

- **CPU raster (softbuffer + tiny-skia) instead of wgpu/GPU.** wgpu cold-init
  on NVIDIA Wayland costs **50–150 ms** of driver and swapchain setup before
  the first frame. For static text-only content, CPU rasterising into a wl_shm
  buffer is essentially free — `surface_ready` lands ~3 ms after window
  creation. *Saves 50–150 ms.*

- **Defer syntect to the second frame.** Frame 1 paints code blocks as plain
  monospace (geometry is identical), presents in ~14 ms, then schedules a
  re-layout that swaps in the highlighted buffers. Doing it inline blocks
  first paint for **~60 ms**. *Saves ~50 ms* off perceived latency.

- **Parallel syntect precompute, one worker per code block.** Different
  languages compile their regex state machines independently, so on a
  multi-core box 3 blocks finish in roughly the time of the slowest single
  one (~25 ms) rather than summed (~75 ms). *Saves ~50 ms* on frame 2.

- **Memoize syntect output by `(lang, code, theme)`.** Without it, every
  resize / zoom / theme toggle re-ran the highlighter for **~60 ms**. With
  it, cached relayouts complete in **<1 ms**. *Saves ~60 ms per interaction* —
  the difference between window resize stuttering and feeling instant.

- **Active-theme-only precompute at startup.** The inactive theme is filled
  lazily on first `t` press. *Saves ~25 ms* at startup; the first toggle
  eats a one-time ~25 ms compile and is cached after.

- **Tight rasterizer loop.** `cosmic-text`'s `Buffer::draw` callback is
  per-pixel; the inner blend was stripped to a single inline call with a
  fast path for the common opaque-destination case (skips post-blend
  premultiplication). *Saves ~1–2 ms* in first-paint.

- **`MDV_TRACE` markers from line 1 of `main()`.** Every choice above was
  decided from the trace output, not guesswork. Run `MDV_TRACE=1 mdv file.md`
  to see the per-stage breakdown on your hardware.
