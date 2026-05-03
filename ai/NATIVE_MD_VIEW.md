# Native vmd — Implementation Plan

Replace the Tauri webview with a native Rust binary to cut cold launch from
~150–250ms to <30ms while preserving feature parity.

## Goals & non-goals

**Hard goals**
- Cold launch (binary exec → first painted frame) < 30ms on this machine
- Feature parity with current vmd (keybinds, themes, zoom, copy, link-out)
- Single static binary, no JS/TS/pnpm/Tauri
- Visual fidelity close to current GitHub-flavored output

**Non-goals (drop deliberately)**
- Math (KaTeX), Mermaid, raw HTML passthrough — already stripped today
- Live reload — separate project if ever wanted
- Print, PDF export
- Accessibility tree (defer; revisit if it becomes a need)

## Current feature inventory (from the Tauri build)

Parser: pulldown-cmark with TABLES, STRIKETHROUGH, TASKLISTS, FOOTNOTES,
HEADING_ATTRIBUTES, SMART_PUNCTUATION.
Render: syntect (default-fancy) classed HTML + GH light/dark CSS.
Chrome: theme auto + manual (persisted); zoom 0.5–3.0 (persisted); copy
buttons on code blocks; external links via xdg-open.
Keybinds: q t +/-/0 j/k d/u f/b g/G ] [ } { ? esc.
CLI: `vmd <file.md | ->`.

## Stack recommendation

GPU init on NVIDIA Wayland is the very thing currently making the webview feel
slow; using wgpu/vello reintroduces 50–150ms of driver warmup and won't beat
the goal. For text-only static documents, CPU rasterization is the right
answer.

| Layer | Choice | Why |
|---|---|---|
| Window/event loop | `winit` 0.30 | Mature Wayland support; new `ApplicationHandler` API |
| Surface | `softbuffer` | Pixel buffer to Wayland surface; ~zero init cost |
| 2D raster | `tiny-skia` | Fast, no_std-friendly, quality on par with skia for text/shapes |
| Text shaping/layout | `cosmic-text` | System fontdb, swash shaping, emoji+CJK fallback, tested at scale |
| Markdown parse | `pulldown-cmark` (keep) | Already in use, fast |
| Syntax highlighting | `syntect` (keep, but use tokens, not HTML) | Reuse existing config |
| Clipboard | `arboard` | Wayland-aware |
| URL open | `opener` crate or direct `xdg-open` exec | Replaces tauri-plugin-opener |

Alternative considered: GTK4 + Pango/Cairo — slower init, heavier deps, but
better native a11y. Not worth it for this goal.
Alternative considered: egui — text quality and selection ergonomics worse
than cosmic-text; skipped.

## Architecture

Pipeline:

```
src ──▶ pulldown-cmark events ──▶ DocAst (typed blocks/inlines)
                                       │
                                       ▼
                         layout(viewport_w, theme, zoom)
                                       │
                                       ▼
                       LaidDoc { blocks: Vec<LaidBlock> }
                                       │
                                       ▼  on each frame
                                  paint(viewport, scroll_y) ──▶ pixmap ──▶ softbuffer
```

Key invariants:
- Layout is recomputed only on width/zoom/theme change. Scroll never re-layouts.
- Each `LaidBlock` carries y-offset, height, and pre-shaped text runs. Painting
  is O(visible blocks).
- Damage region tracked: scroll only blits + paints incoming strip; full
  repaint only on width/zoom/theme change.

## File layout (proposed)

Reuse `src-tauri/` as the new crate (rename to top-level, demote `src-tauri/`
once Tauri is gone). New module tree:

```
src/
  main.rs         # CLI + winit app bootstrap
  app.rs          # ApplicationHandler, event routing, redraw scheduling
  doc.rs          # DocAst types + builder from pulldown-cmark events
  layout/
    mod.rs        # entry: DocAst -> LaidDoc
    block.rs      # heading/para/list/quote/hr/code/footnote layout
    inline.rs     # styled run construction, line breaking via cosmic-text
    table.rs      # column measure + row layout
    metrics.rs    # font sizes, paddings, indents per theme
  paint.rs        # tiny-skia draw of LaidBlock {y..y+h} into pixmap
  text.rs         # FontSystem + SwashCache singletons; helpers
  highlight.rs    # syntect SyntaxSet/ThemeSet → styled spans (no HTML)
  theme.rs        # GH light/dark palettes; resolves style at use site
  scroll.rs       # scroll_y, target, anim; jump-to-heading/block
  input.rs        # keymap dispatch (mirrors current main.ts)
  clipboard.rs    # copy code block / selection
  opener.rs       # external URL handoff
  state.rs        # persisted prefs (theme, zoom) under XDG state dir
assets/
  Inter-Regular.ttf       # bundled, OFL 1.1
  Inter-Bold.ttf
  Inter-Italic.ttf
  Inter-BoldItalic.ttf
  Inter-OFL.txt           # license, shipped with binary via include_str!
  JetBrainsMono-Regular.ttf
  JetBrainsMono-Bold.ttf
  JetBrainsMono-Italic.ttf
  JetBrainsMono-OFL.txt
```

Drop: `src/` (TS), `index.html`, `package.json`, `vite.config.ts`,
`tsconfig.json`, `node_modules/`, `dist/`. Move `src-tauri/Cargo.toml` to root
after tauri deps removed.

## Phased plan

Each phase is a working commit. Estimated effort assumes single dev, focused.

### Phase 0 — Decisions locked, scaffolding (½ day)
- Confirm CPU stack above.
- New `Cargo.toml`: drop `tauri`, `tauri-build`, `tauri-plugin-opener`. Add
  `winit`, `softbuffer`, `tiny-skia`, `cosmic-text`, `arboard`, `opener`,
  `directories`. Keep `pulldown-cmark`, `syntect`, `serde` (only if needed).
- Delete `build.rs`, `tauri.conf.json`, `capabilities/`, `gen/`, `icons/` (or
  keep one PNG for window icon).
- **Bundle fonts** under `assets/` (see [Appendix B](#appendix-b--font-bundling-and-licensing)
  for rationale and obligations):
  - Inter Regular/Bold/Italic/BoldItalic — SIL OFL 1.1
  - JetBrains Mono Regular/Bold/Italic — SIL OFL 1.1
  - The two `OFL.txt` files **must** ship with the binary. Embed via
    `include_str!` and expose via `vmd --licenses`.
- `VMD_TRACE` env stays, with `Instant::now()` markers at the same checkpoints
  as `lib.rs:23-31`.
- **Exit criteria**: `cargo build --release` produces a stub binary; old
  keybind/CLI usage docs preserved; `vmd --licenses` prints both OFL texts.

### Phase 1 — Window + first paint (1 day)
- `app.rs`: winit `ApplicationHandler`, single window, softbuffer surface,
  tiny-skia pixmap sized to surface.
- `text.rs`: build `fontdb::Database` manually from `include_bytes!` of
  bundled TTFs. **Never call `db.load_system_fonts()`** — it triggers a
  fontconfig-equivalent scan that costs ~50–150ms (see Appendix A.1).
  Construct via `FontSystem::new_with_locale_and_db(locale, db)`.
- Render a centered "vmd" string. Measure cold-start total. Run with
  `VMD_TRACE=1` to attribute time across stages.
- **Exit criteria**: < 10ms exec→first paint on this hardware (Wayland, NVIDIA,
  9800X3D). Treat as a research spike — if missed by more than 5ms,
  investigate before proceeding rather than relying on later optimization.
  See Appendix A.10 for the fallback architecture.

### Phase 2 — DocAst + minimal block render (1 day)

Define `DocAst`:

```rust
enum Block {
    Heading { level: u8, id: String, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    List { ordered: bool, start: u64, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    CodeBlock { lang: String, code: String },
    Rule,
    Table { headers: Vec<Vec<Inline>>, aligns: Vec<Align>, rows: Vec<Vec<Vec<Inline>>> },
    Footnotes(Vec<(String, Vec<Block>)>),
    TaskItem { checked: bool, inlines: Vec<Inline> },
}
enum Inline {
    Text(String), Code(String), Strong(Vec<Inline>), Em(Vec<Inline>),
    Strike(Vec<Inline>), Link { href: String, kids: Vec<Inline> },
    Image { src: String, alt: String }, FootnoteRef(String), HardBreak, SoftBreak,
}
```

- Build it from `pulldown-cmark` events (port `transform` from `render.rs:34`;
  this becomes much cleaner without HTML emit).
- Layout headings + paragraphs only; everything else collapses to a placeholder
  block with a label.
- **Exit criteria**: `examples/test.md` shows headings and paragraphs with
  correct wrapping at window width.

### Phase 3 — Inline layout via cosmic-text (1.5 days)
- One `cosmic_text::Buffer` per block, attrs spans for bold/italic/strike/code/link.
- Link styling = colored + underline; record per-glyph `range -> href` map for
  hit testing.
- SoftBreak → space, HardBreak → newline; smart punctuation already done by
  parser.
- **Exit criteria**: paragraphs with mixed inline styles render correctly;
  resizing window reflows.

### Phase 4 — Lists, blockquote, hr, task lists (1 day)
- Lists: nested indent (e.g. 1.5em); ordered uses parser's `start`; bullets
  manually drawn.
- Quote: left bar + indented child blocks (recurse).
- HR: 1px line at theme separator color.
- Task list: drawn checkbox glyph, no toggling (read-only viewer).
- **Exit criteria**: matches current GH-style spacing within a few px.

### Phase 5 — Code blocks with syntect (1 day)
- Convert syntect `Style` to cosmic-text `Color`+weight per token range.
- Background fill, rounded corners (tiny-skia path), language label top-right
  (style.css:40).
- Horizontal overflow: paint clipped, draw a faint right edge fade (cheap
  visual cue) instead of a real horizontal scrollbar (defer scrollbar to v1.1).
- Inline `code` spans: monospace + background pill.
- **Exit criteria**: `examples/test.md` code blocks readable, light/dark themes
  both look right.

### Phase 6 — Tables, footnotes, images (1.5 days)
- Tables: two-pass layout — measure max content width per column (capped),
  distribute remainder; per-row max height; align via `Align`.
- Footnotes: render as definition list at end; `FootnoteRef` is a superscript
  link to the def.
- Images: `image` crate (PNG/JPG/GIF first frame). Local paths resolved
  relative to source file. Remote URLs: alt text only in v1; fetch behind a
  flag in v1.1.
- **Exit criteria**: visual diff vs Tauri build is acceptable on a
  representative doc.

### Phase 7 — Theming (½ day)
- `theme.rs` with two const palettes mirroring `gh-light.css` / `gh-dark.css`
  (text, muted, link, code-bg, border, table-row-zebra, etc.).
- Background painted by tiny-skia clear, not CSS.
- Auto detect via `dark-light` crate (XDG portal on Wayland) or env var
  fallback; toggle with `t`, persist to state file.
- **Exit criteria**: `t` cycles instantly with no flash.

### Phase 8 — Scroll + jump motions (1 day)
- `scroll.rs`: `scroll_y: f32`, target tween with simple ease (skip if user
  wants instant — current TS uses `behavior: "instant"`, mirror that). Bound
  to `[0, total_h - viewport_h]`.
- Mouse wheel via winit `MouseWheel`.
- Keybinds (`input.rs`):
  - j/k → SCROLL_LINE_PX (40)
  - d/u → 0.5 · viewport_h
  - f/b → 0.9 · viewport_h
  - g/G → top/bottom
  - `]` `[` → next/prev heading (binary search over heading y-offsets)
  - `}` `{` → next/prev top-level block
  - `+`/`-`/`0` → zoom (triggers re-layout)
  - `q` → exit; `t` → theme toggle; `?` → help overlay
- **Exit criteria**: every Tauri keybind works and feels at least as snappy.

### Phase 9 — Help overlay, copy, link-out (½ day)
- Help overlay: paint a centered rounded rect with the same table from
  `index.html:32-46`. Trivially layoutable as a small DocAst snippet.
- Copy: hover detection on code blocks. With no native cursor in tiny-skia,
  easier to just bind `y` (yank) to copy the code block under the caret
  position. Also expose `c` as "copy block under cursor". Skip the floating
  button — it's a webview convention.
- Link click: Phase 3 already records href ranges. On `LeftMouseUp`, hit test
  against current laid blocks; if hit, `opener::open(href)`.
- **Exit criteria**: parity-or-better on chrome.

### Phase 10 — Selection + clipboard (1 day, **optional but high value**)
- cosmic-text exposes glyph runs with byte ranges; track `(start, end)`
  selection in document coordinates.
- Drag to select, double-click word, triple-click line/paragraph.
- `Ctrl+C` → copy selected plain text.
- This is the one feature where users will notice "oh this isn't a webview" if
  missing. Worth the day.

### Phase 11 — Polish (1 day)
- HiDPI: respect `Window::scale_factor()`; layout in logical px, paint in
  physical px.
- syntect theme preload: load InspiredGitHub + base16-ocean.dark on a thread
  before window open (mirror existing `lib.rs:53` pattern). Note: fonts no
  longer need preloading since they're bundled.
- Window title `"<filename> — vmd"` (already done in `lib.rs:78`).
- Window icon (one tiny PNG embedded via `include_bytes!`).
- Stdin path: read fully before window create, same as today.
- Error path: render error message to a doc and show it (don't `eprintln +
  exit` once window exists).
- `--licenses` flag wired (already added in Phase 0).
- README mentions bundled fonts and links to OFL (Appendix B obligation).
- **Exit criteria**: looks good on this 5090 + Wayland setup at default scale
  and zoomed.

### Phase 12 — Replace install path (½ day)
- `install.sh`: `cargo build --release`; symlink `target/release/vmd` to
  `~/.local/bin/vmd`. Drop the `WEBKIT_DISABLE_DMABUF_RENDERER` wrapper — no
  longer needed.
- `vmd.desktop` unchanged.
- Delete `src/` (TS), `index.html`, `package.json`, `pnpm-lock.yaml`,
  `vite.config.ts`, `tsconfig.json`, `node_modules/`, `dist/`. Move Rust crate
  to repo root.
- **Exit criteria**: `vmd examples/test.md` uses the native binary; old build
  artifacts gone.

## Performance budget (commit to numbers)

Calibrated against tofi's published numbers (Ryzen 7 3700X dmenu = 2.3ms; our
9800X3D ≥ 3700X; our 920×1100 window ≈ 1MP, between tofi's 60px ribbon and
fullscreen). With bundled fonts (no system scan) the budget tightens
considerably:

| Stage | Target | Notes |
|---|---|---|
| `main()` → `winit::run()` | < 1ms | Just CLI + file read |
| File read | < 1ms | typical README |
| Parse + DocAst build | < 2ms | pulldown-cmark is fast |
| Font load (bundled, no scan) | < 2ms | `include_bytes!` + fontdb add |
| Initial layout (offscreen) | < 4ms | background thread during window create |
| Window create + first paint | < 6ms | softbuffer+tiny-skia, ~1MP surface |
| **Total exec → visible** | **< 10ms** | vs current 150–250ms |
| Re-layout on resize/zoom | < 10ms | for 5k-line doc |
| Scroll repaint | < 2ms | partial repaint, viewport-only |

If Phase 1 measurement misses < 10ms by more than 5ms, the most likely cause
is cosmic-text shaping overhead per paragraph (it does more work than tofi's
single-line shaper). Fallback architecture in Appendix A.10.

## Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| cosmic-text paragraph shaping overhead (not apples-to-apples with tofi's single-line shaper) | Medium | Measure in Phase 1; cache shaped buffers across re-paints; A.10 fallback if too heavy |
| Selection ergonomics are subtly wrong (BiDi, ligatures) | Medium | Use cosmic-text's selection helpers, don't reinvent |
| Table layout edge cases (very long cells) | Medium | Cap measured col width, allow horizontal overflow with fade |
| Bundled fonts lack a glyph user expects (e.g. CJK in headings) | Low | Document limitation; allow `--font-sans /path` override; later: optional Noto fallback |
| NVIDIA + Wayland softbuffer quirks | Low | Already proven; if buggy, fall back to wgpu later |
| Image loading blocks startup | Low | Lazy decode on first scroll into view |
| Dark/light mode flicker on toggle | Low | Re-layout only inline color attrs change, not geometry — flag in `theme.rs` |
| ~~`FontSystem::new()` system scan is slow~~ | ~~High~~ → resolved | Bundled fonts via `include_bytes!`, no system scan ever runs (A.1) |

## Migration

- Branch `native`. Keep `main` on Tauri until Phase 11.
- Phase 0–2 are reversible — don't delete the Tauri tree until Phase 6
  demonstrates the approach can carry the rest of the features.
- `VMD_TRACE` markers stay so we can diff old vs new launch profile.

## Open questions

1. **Selection** (Phase 10) — keep or skip? Highest-effort polish item.
2. **Image fetch** for remote URLs — never, on-demand, or behind a flag?
3. **Theme** — match GH exactly, or take the chance to design something a
   touch nicer (e.g. better code block bg, slightly tighter line-height)?
4. **Window decorations** — server-side default, or borderless minimal?
5. **Mouse-mode copy button** vs the proposed `y`/`c` keybind — webview
   convention vs vim convention.

Total estimate: ~10–12 focused days for full parity + selection. ~7 days if
Phase 10 is skipped.

## Appendix A — Tofi-inspired optimizations

[philj56/tofi](https://github.com/philj56/tofi) is a Wayland launcher that
gets on screen in ~2ms. Its README documents exactly where the time goes,
and most of its tricks transfer directly. Folded into the relevant phases:

### A.1 Bypass fontdb system scan via bundled fonts (Phase 0/1, **highest impact**)

Tofi measured ~120ms for a single Pango+fontconfig font lookup against ~10k
system fonts; a direct path to a TTF drops that to <1ms. `FontSystem::new()`
in cosmic-text does the equivalent system scan and is the single biggest
startup cost we'd otherwise pay.

**Plan — bundle fonts in the binary**:
- Build `fontdb::Database` manually. Do **not** call `db.load_system_fonts()`.
- Embed font bytes at compile time:
  ```rust
  const INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
  const INTER_BOLD:    &[u8] = include_bytes!("../assets/Inter-Bold.ttf");
  // …
  let mut db = fontdb::Database::new();
  db.load_font_data(INTER_REGULAR.to_vec());
  db.load_font_data(INTER_BOLD.to_vec());
  // …
  let fs = FontSystem::new_with_locale_and_db("en-US".into(), db);
  ```
- Binary size cost: ~1–2 MB total for Inter (4 styles) + JetBrains Mono
  (3 styles). Acceptable for a desktop app.
- License obligations: see [Appendix B](#appendix-b--font-bundling-and-licensing).
- CLI override for users who want a different font:
  `vmd --font-sans /path --font-mono /path file.md` — when supplied, replaces
  the bundled font in the database.

**Why bundling beats `fc-match` at install time**:
- No install-time step (no fontconfig dependency at install).
- Deterministic: same fonts regardless of host config.
- No config file to maintain at runtime.
- Works the same on every machine (this one, friend's, server, container).

Expected impact: cuts the dominant chunk of cold start. Combined with A.2,
puts the < 10ms target within reach.

### A.2 Disable hinting at native scale (Phase 1 / Phase 5)

Tofi: hinting on first text render = 4–6ms; off = ~1ms. On a HiDPI display
(this 5090 setup typically runs > 1× scale) hinting is mostly noise anyway.

**Plan**:
- Default `Attrs::hinting(Hinting::None)` — actually exposed in swash via
  `ScaleContext` / `Render::hint(false)`.
- Add `--hint` flag for users who want it back.

### A.3 Late keyboard init (Phase 8)

Tofi defers xkb context creation until after first paint to shave 1–2ms (up
to 60ms on slow hardware). winit doesn't expose this directly, but we can:

- Skip our keymap setup until after the first `RedrawRequested` completes.
- Buffer any `KeyboardInput` events that arrive in the gap and replay once
  ready. Risk: a keypress in the first ~5ms is dropped — acceptable, this is
  a viewer not an input app.

### A.4 memfd_create + transparent hugepages (Phase 11, Linux only)

softbuffer wraps wl_shm via the wayland-backend crate. For our buffer size
(window-sized RGBA), hugepages reduce first-paint page faults.

- If softbuffer's allocator already uses `memfd_create`, nothing to do.
  Verify in Phase 1.
- If not, fork the buffer alloc to use `memfd_create("vmd_shm", 0)` directly,
  matching tofi's `src/shm.c`.
- Document the hugepages tuning (`/sys/kernel/mm/transparent_hugepage/shmem_enabled = advise`)
  in README as opt-in, like tofi does. Don't require it.

### A.5 Surface-size-aware first paint (Phase 8)

Tofi: fullscreen first paint ~20ms vs 1ms for a ribbon. Our window is
naturally larger than tofi's, so we should expect ~10–20ms for first paint
of a typical viewer window. Mitigations:

- First frame paints **only the viewport** of the doc, not the offscreen
  cache. Already implied by the architecture, but make this explicit: the
  laid-out blocks below the fold should not be rasterized in frame 1.
- Avoid the temptation to pre-render to a full-document offscreen pixmap.
  Composite per-frame from `LaidBlock` shaped buffers instead.
- Use `wp_viewporter` if available so we can blit a smaller buffer scaled to
  the surface, matching tofi's approach for HiDPI without re-rendering.

### A.6 Two-buffer initialization (Phase 11)

Tofi initializes one buffer, paints, sends to compositor, *then* initializes
the second buffer for double-buffering. Mirror this:

- Buffer 0: allocated and painted on the critical path to first frame.
- Buffer 1: allocated after `RedrawRequested` returns, before the next event.

### A.7 Trace-from-line-one discipline

Tofi's first instruction is essentially `log_debug("This is tofi.\n")`,
which initializes the perf timer. Match this:

- Move `APP_START` initialization to the very first line of `main()`,
  before argv parsing.
- Keep `VMD_TRACE` markers at every phase boundary (file read, parse, layout
  start/done, fontdb ready, window create, first paint).
- Add a `--trace` CLI flag that's equivalent to `VMD_TRACE=1` for ergonomic
  benchmarking.

### A.8 Things tofi does that we should *not* copy

- `--ascii-input` (skip Unicode) — markdown is inherently Unicode (smart
  punctuation, em dashes, code identifiers). Not safe to skip.
- Daemon mode — tofi rejects this for complexity; for a markdown viewer the
  one-shot model is correct, no shared state to keep warm.
- Custom shm allocator from scratch — only fork from softbuffer if profiling
  shows it's needed.

### A.9 Updated performance budget

With A.1+A.2+A.3 the realistic target tightens to **< 10ms**, in the same
neighborhood as tofi-dmenu on equivalent (slower) hardware. See the main
[Performance budget](#performance-budget-commit-to-numbers) table for the
breakdown.

### A.10 Fallback architecture if cosmic-text is too heavy

cosmic-text does **paragraph-level shaping with line breaking and Unicode
segmentation**; tofi does **single-line shaping of a known string**. These
are not the same workload. A markdown doc with 200 paragraphs is doing 200
shape+break passes at first layout — that's not free even with bundled fonts.

If Phase 1 spike misses the < 10ms budget by more than ~5ms and profiling
points at cosmic-text, the fallback is to mirror tofi's pipeline more
literally:

- **Shaping**: `harfbuzz_rs` directly, one HarfBuzz buffer per styled run.
- **Rasterization**: `swash` (already a cosmic-text dependency) or `fontdue`
  for glyph caching, drawn into the tiny-skia pixmap.
- **Line breaking**: hand-roll a simple greedy breaker — adequate for body
  text, no BiDi support needed for our content, no hyphenation.
- **Selection**: harder without cosmic-text's helpers; would push Phase 10
  effort up by ~1 day.

Don't pre-emptively drop cosmic-text — it gives us selection, mixed scripts,
and emoji fallback for free. But know the escape route.

## Appendix B — Font bundling and licensing

The bundled-font approach (A.1) lets us delete the dominant startup cost,
but it ships third-party files inside the binary, so the licenses must be
respected. Both fonts chosen are under **SIL Open Font License 1.1** — a
permissive license designed exactly for this use case.

### Fonts and their licenses

| Font | License | Source |
|---|---|---|
| Inter (Regular/Bold/Italic/BoldItalic) | SIL OFL 1.1 | https://github.com/rsms/inter |
| JetBrains Mono (Regular/Bold/Italic) | SIL OFL 1.1 | https://github.com/JetBrains/JetBrainsMono |

Earlier I incorrectly described JetBrains Mono as Apache 2.0; it is OFL 1.1.

### What OFL 1.1 permits

- ✅ Embed the font in a binary (including via `include_bytes!`).
- ✅ Redistribute, freely or commercially, as part of software.
- ✅ Bundle with closed-source software (OFL is **not** copyleft for the
  software that embeds the font).
- ❌ Sell the font file by itself (irrelevant for vmd).
- ❌ Ship a modified font under its Reserved Font Name (e.g. a tweaked Inter
  cannot still be called "Inter").

### Hard requirements we must meet

1. **Ship the OFL text alongside the binary.** Two acceptable approaches,
   pick one (or both):
   - Embed via `include_str!("../assets/Inter-OFL.txt")` and expose
     `vmd --licenses` (preferred — license travels with the binary).
   - Keep `assets/*-OFL.txt` in the repo and reference them from README.
2. **Preserve copyright + Reserved Font Name notices** that appear in the
   OFL files. Don't strip them when copying into `assets/`.
3. **Don't modify the font files.** Subsetting for size counts as
   modification — if we ever do that, rename the subset (e.g. `MdvSans.ttf`)
   so we're not shipping something called "Inter" that isn't Inter. For the
   unmodified upstream `.ttf`s, no renaming needed.
4. **Mention the fonts and their licenses somewhere user-visible** (README is
   enough).

### Concrete obligations checklist

- [ ] `assets/Inter-OFL.txt` and `assets/JetBrainsMono-OFL.txt` in the repo.
- [ ] `include_str!` of both license texts in `src/main.rs` (or `src/licenses.rs`).
- [ ] `--licenses` CLI flag prints both, then exits 0.
- [ ] README has a "Bundled fonts" section listing both fonts, license, and
      upstream URLs.
- [ ] No modifications to the `.ttf` files. If a future task subsets them,
      the subset gets a new name.

### Things to avoid bundling

- **Apple system fonts** (San Francisco) — strict EULA, never bundle.
- **Microsoft fonts** (Segoe UI) — strict EULA. (Cascadia Code ships
  separately under OFL and is fine, but isn't the same font.)
- **Anything from `~/.local/share/fonts/`** picked up blindly — many users
  have commercial fonts there. Hard-code specific known-OFL files in
  `assets/` instead.

### If we add emoji fallback later

Noto Color Emoji is also OFL 1.1 — same rules apply, add a third
`NotoColorEmoji-OFL.txt`. Color emoji adds ~10MB to the binary, which is why
it's not in the v1 plan.
