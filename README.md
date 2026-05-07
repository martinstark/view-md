# view md

A fast, native viewer for markdown and JSON, on Linux, macOS, and Windows.

Renders rich layout in a single 120 Hz frame (<8.3 ms).

Vim/vimium-style keybinds: `y` yanks code, `f` opens links, `/` searches.

Light and dark themes.

Purpose built for a terminal + browser workflow.

![demo2](assets/demo2.gif)

## JSON / JSONC / JSON5

`vmd` doubles as a JSON viewer. Pass a `.json` / `.jsonc` / `.json5`
file (or pipe JSON to stdin, or pass `--json` to force the mode) and
the file is reformatted with canonical 2-space indentation and rendered
as a single syntax-highlighted block. JSONC and JSON5 input is accepted —
comments, trailing commas, unquoted keys, single quotes, hex literals,
±Infinity / NaN — but comments are dropped from the rendered view.

`f` opens vimium-style hint mode: each key gets one badge (copies the
key name) and each value gets one (copies the literal for primitives,
or the formatted subtree for objects and arrays). Invalid JSON exits
non-zero with a `line:col` error.

Try the included stress demo:

```sh
vmd examples/big.json   # 770 KB, ~35 k lines
```

The "single 120 Hz frame" budget applies to typical README-sized docs.
Multi-megabyte JSON files take longer — cold launch on the 770 KB
demo is ~800 ms on a 9800X3D, dominated by cosmic-text shaping and
softbuffer rasterization of every line.

## Why

Inspired by the raw speed of [tofi](https://github.com/philj56/tofi), a launcher that can open in a single frame.

Quickly jump in and out of markdown files to check their contents: README.md, SKILL.md, PLAN.md...

"I" created this tool to make it as seamless and painless as possible.

## Here be AI

For your own sanity, do not read the source code. All planning docs and messy git history included for full transparency. This repository does not represent my personal code standards.

## Build

    cargo build --release
    # symlink target/release/vmd to ~/.local/bin
    # installs .desktop metadata entry
    ./install.sh

## Use

```sh
vmd file.md
vmd file.json         # JSON / JSONC / JSON5
vmd 'file.md#section' # open at anchor
vmd -                 # read from stdin (sniffs JSON vs markdown)
vmd --json -          # force JSON mode on stdin
vmd --licenses        # print vmd's license + bundled fonts + all third-party deps
vmd --trace           # print timing breakdown
vmd --watch file.md   # watches file for changes and live updates
```

`?` to show keybinds.

`f` to interact.

`/` to search.

`q` to quit.

`+`, `-` or `0` to scale.

`j`, `k`, `d`, `u` to navigate.

## License

vmd is dual-licensed under MIT or Apache-2.0; see `LICENSE-MIT` and
`LICENSE-APACHE`. Run `vmd --licenses` (or read `THIRD-PARTY-LICENSES.md`)
for the full text of every embedded dependency.

To regenerate `THIRD-PARTY-LICENSES.md` after a `cargo update` or new dep:

    cargo install cargo-about --features cli   # one-time
    cargo about generate about.hbs > THIRD-PARTY-LICENSES.md

## Bundled Fonts

vmd embeds the following fonts to skip fontconfig at startup. Both are under
the SIL Open Font License 1.1. See `vmd --licenses` or the files in `assets/`
for the full text.

- Inter (Regular, Bold, Italic, BoldItalic). © 2016 The Inter Project Authors. https://github.com/rsms/inter
- JetBrains Mono (Regular, Bold, Italic). © 2020 JetBrains s.r.o. https://github.com/JetBrains/JetBrainsMono

## How it's Fast

- bundled fonts, zero-copy. Skips fontconfig (50 to 150 ms with ~10k fonts installed)
- CPU raster, no GPU. Skips ~50 to 150 ms of gpu driver init
- mimalloc as global allocator
- parse, then everything else in parallel:
    - speculative layout + shape on a background thread
    - pre-warm the swash glyph cache for the visible viewport
    - syntax highlight parsing
- draw without waiting for syntax highlighting thread if no visible code block on first frame
- skip the `request_redraw` round-trip
- glyph raster via `swash.get_image()`
- memoize highlights by `(lang, code, theme)`
- process active theme only at startup

A typical README cold-launches inside one 120 Hz frame (<8.3 ms exec → present), with one caveat: if there are code blocks in the initial visible frame the launch waits for syntect to finish computing highlights to avoid a redraw. Worst case, this delays launch by one extra frame (~5 ms).

## Trace

Measured on a Ryzen 9 9800X3D, Wayland/SwayWM, against `examples/test.md`.

```md
❯ VMD_TRACE=1 target/release/vmd examples/test.md
[vmd]   0.006ms main
[vmd]   0.033ms source_read
[vmd]   0.038ms run_start
[vmd]   0.121ms fontsystem_ready
[vmd]   0.284ms doc_parsed
[vmd]   0.290ms image_dims_read n=0
[vmd]   0.320ms layout_workers_ready
[vmd]   0.325ms syntect_warm_start
[vmd]   1.049ms syntect_defaults_ready
[vmd]   2.203ms event_loop_created
[vmd]   2.210ms resumed
[vmd]   2.546ms speculative_layout_done
[vmd]   3.013ms window_created
[vmd]   3.039ms clipboard: bound to window's wl_display
[vmd]   3.494ms surface_ready
[vmd]   4.839ms speculative_warm_done
[vmd]   4.880ms speculative_layout_joined
[vmd]   4.886ms speculative_layout_used
[vmd]   4.889ms layout_ready
[vmd]   4.893ms syntect_wait_skipped (no placeholder code in viewport)
[vmd]   4.896ms redraw_first
[vmd]   6.431ms first_present                          <-- markdown rendered
[vmd]   7.737ms relayout_full_highlight
[vmd]   7.808ms syntect_precompute_done                
[vmd]   8.750ms relayout_full_highlight_done           <-- syntax highlight redraw
```
