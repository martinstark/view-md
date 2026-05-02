# vmd launch/render performance — analysis & options

Baseline measurements with `VMD_TRACE=1` on this machine (9800X3D, 5090,
Wayland/sway).

## Where time goes today

`examples/test.md` (12 code blocks, 268 lines):

```
  0.5ms  fontsystem_ready       7 fonts loaded into fontdb
  0.6ms  doc_parsed             pulldown-cmark
  0.7ms  syntect spawn (bg)
  2.3ms  event_loop_created     winit/Wayland
  3.1ms  window_created
  3.2ms  surface_ready
  8.0ms  layout_ready           ~5ms: cosmic-text shaping every block (single thread)
 12.4ms  redraw_first
 20.5ms  first_present          ~8ms: glyph raster + per-pixel blend + BGRA→RGB
 21.2ms  relayout_full_highlight
 40.9ms  syntect_precompute_done
 45.5ms  relayout_full_highlight_done   ~24ms: re-shapes ENTIRE doc, not just code
```

`README.md` (no code blocks): first paint 17.8ms, full upgrade 21ms.

## Top opportunities, ranked

### 1. Don't re-lay out the whole doc just to refresh syntax highlighting ✅ DONE
**Saves 10–25ms on frame 2.** Single biggest win.

`app.rs:574` (`upgrade_pending` path) calls full `relayout`, re-shaping every
paragraph, list, table, heading — none of which changed between frame 1 and
frame 2. Only code-block buffers differ (placeholder spans → highlighted
spans).

Fix: keep block indices for `LaidKind::CodeBlock` only and rebuild just those
buffers in place. Rest of the laid doc is untouched.

### 1b. Trigger the upgrade off precompute completion (and skip it entirely when precompute beats first paint) ✅ DONE
A refinement of (1). The precompute thread now sets an `Arc<AtomicBool>`
when its cache is warm. `App::relayout` checks it and lays out with full
highlighting from the start when set — so on no-code or fast-precompute
docs (e.g. README.md) the second pass never fires at all. The post-first-
paint auto-trigger from (1) is kept for code-heavy docs, where firing
immediately lets cache-hit code blocks be re-shaped in parallel with any
remaining precompute work (this beat the alternative of waiting for a
proxy/UserEvent — empirically the wait was ~3ms slower on test.md).

Measured (n=20 each, fully_rendered = max(first_present, relayout_done)):

| doc | baseline | after (1+1b) | delta |
|---|---:|---:|---:|
| README.md (no code) | 37.40ms | 32.54–33.89ms | **−3.5 to −4.9ms (−10–13%)** |
| examples/test.md (12 code blocks) | 51.86ms | 50.15–51.22ms | −0.6 to −1.7ms |

README never runs the upgrade pass; test.md still runs it but on code
blocks only.

### 2. Parallelize per-block shaping during layout
**Saves 3–5ms on frame 1.**

`layout()` walks blocks sequentially through `&mut FontSystem`. cosmic-text
Buffers are independent. Build N worker `FontSystem`s from a shared
`Arc<fontdb::Database>` (the db is the heavy part — `FontSystem` itself is
cheap), shape blocks in parallel via rayon or hand-rolled threads, collect.
With ~16 cores and ~50 blocks, layout drops from ~5ms toward ~1ms.

### 3. Avoid the 2.8MB font memcpy at startup
**Saves ~0.3ms.**

`text.rs:16` does `db.load_font_data(INTER_REGULAR.to_vec())` for 7 fonts —
allocates and copies ~2.8MB from `.rodata` into the heap. Use
`fontdb::Source::Binary(Arc::new(STATIC_SLICE))` to point at static data
directly.

### 4. Swap the allocator (mimalloc)
**~5–10% across the board.**

cosmic-text + tiny-skia + syntect do many small allocs during shaping/raster.
One-line `#[global_allocator]` in `main.rs`. Free, almost no risk.

### 5. `opt-level = "s"` is sized for the wrong axis
Cargo profile is tuned for binary size. Switch to `opt-level = 3` (or add a
separate `release-perf` profile). Hot loops in `paint.rs` (`blend_pixel`,
`pixmap_to_softbuffer`, glyph rasterization) benefit most. Costs maybe 1–2MB
binary.

### 6. Speed up per-pixel hot loops in `paint.rs`
- `pixmap_to_softbuffer` (paint.rs:710) is a per-pixel scalar copy. Process 4
  pixels/iteration with `chunks_exact`, use bit shifts, or pull in `bytemuck`
  + SIMD.
- `blend_pixel` (paint.rs:666) is called once per glyph pixel. Batch
  contiguous horizontal runs from the same glyph into a single span blend.
  Could halve paint time on text-heavy docs.

### 7. Start window/surface creation in parallel with layout
**Saves ~4ms on the critical path.**

In `resumed()`: window create (1ms) → surface ready → layout (5ms), serial.
Spawn layout with assumed default width (920) on a thread the moment `doc` is
parsed; await it in `resumed()` and only re-layout if actual width differs.

### 8. Cache `LaidDoc` to disk keyed by (path, mtime, theme, zoom, dpi, width)
For "open the same README repeatedly", deserializing a laid-out doc could be
≤2ms vs ~10ms re-layout. Higher complexity (cosmic-text Buffers aren't
trivially serializable — would need to store the styled-run plan + recompute
shaping, or store glyph positions). Big win for repeat opens, considerable
engineering cost.

### 9. Lazy/deferred shaping past viewport
`shape_until_scroll(fs, false)` shapes the entire buffer up front. For large
docs, shape only what's near the viewport on first paint, finish the rest
after `first_present`. Marginal on small docs, big on multi-MB docs.

## Considered and rejected

### `Shaping::Basic` for ASCII-only spans
Detection is essentially free (`str::is_ascii` is SIMD), but the consequence
isn't acceptable. `Shaping::Basic` skips the OpenType shaper entirely:

- `calt` + `liga` on JBM (code blocks): `->`, `=>`, `!=`, `>=` ligatures lost
- `ss02` on Inter (body): disambiguated `I` / `l` / `1` lost
- Normal kerning pairs (AV, To, fi) silently regress

ASCII-heavy content (especially code) is exactly where the shaper is doing
visible work. Keep `Shaping::Advanced` everywhere.

## Things that could be omitted

- **Bold-italic Inter and italic JBM**: rarely hit. Removing 2 fonts saves
  ~700KB binary and ~50µs load.
- **`syntect default-fancy` → `default`**: the fancy regex engine is heavier;
  standard `default` is enough for the bundled themes/syntaxes. Saves a bit
  of cold-start in `syntaxes()`.
- **Smart punctuation**: a few µs of parser cost. Not worth it for the perf,
  but flag if you don't care about curly quotes.
- **Ligatures (`mono_features`)**: shaping cost in code blocks. Disable to
  speed up code-heavy docs slightly; visible quality regression.

## Single-PR recommendation

Do (1) + (2) + (3) + (4) together. (1) is mostly a refactor in
`app.rs::redraw` and `layout`; (2) requires touching `layout.rs` to
parallelize block shaping with a per-worker `FontSystem` over a shared
`Arc<fontdb::Database>`; (3) and (4) are one-liners. Expected outcome: first
paint ~12ms, "real" paint ~20ms — roughly halving both numbers without
touching the painter or risking regressions in glyph fidelity.
