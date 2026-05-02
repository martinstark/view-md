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

### 2. Parallelize per-block shaping during layout ✅ DONE
**Saves 3–5ms on layout, 5–8ms total wall-clock on frame 1.**

`layout()` walked blocks sequentially through one `&mut FontSystem`. The
new `layout_parallel` round-robin partitions top-level blocks across
`1 + N_LAYOUT_WORKERS` lanes (caller thread + worker threads, each with
its own FontSystem). cosmic-text Buffers are independent per block; std
`thread::scope` lets workers borrow their FontSystem mutably without
heap-allocated handles.

Cross-thread font-id compatibility: each FontSystem is built from
identical font data in identical order, and fontdb stores faces in a
`slotmap` whose keys are deterministic for fresh maps — so a Buffer
shaped on a worker can be painted using the painter's FontSystem
because cache_key.font_id matches.

Worker FontSystems are built on a background thread (~1ms) parallel to
parse + event-loop init, so the cost is off the critical path.

Tunable: `N_LAYOUT_WORKERS` in `lib.rs` (default 2). Bumping trades
~3MB/worker memory for more parallelism on bigger docs.

Measured (n=20 each):

| doc | baseline | after (1+1b) | after (1+1b+2) |
|---|---:|---:|---:|
| README.md | 37.40ms | 32.5–33.9ms | **28.5–29.8ms (−7.6 to −8.9ms, −20–24%)** |
| examples/test.md | 51.86ms | 50.2–51.2ms | **45.7–46.4ms (−5.4 to −6.1ms, −10–12%)** |
| NATIVE_MD_VIEW.md (559 lines) | not measured | not measured | 48.62ms |

### 3. Avoid the 2.8MB font memcpy at startup ✅ DONE
**Saves ~0.35ms on font load (per FontSystem) and 2.8MB heap per FontSystem.**

`text.rs` now wraps `&'static [u8]` in a tiny `StaticFont` newtype that
impls `AsRef<[u8]>`, then uses `db.load_font_source(Source::Binary(...))`
instead of `load_font_data(Vec::to_vec())`. fontdb keeps the Arc'd
reference; no copy.

Measured: `fontsystem_ready` trace point dropped from ~0.46ms to
~0.09ms (~5×). With 3 FontSystems (painter + 2 workers from item 2),
the heap savings is ~9MB.

### 4. Swap the allocator (mimalloc) ✅ DONE
**~1ms across the layout/paint hot path.**

Added `#[global_allocator] static GLOBAL: mimalloc::MiMalloc = MiMalloc`
in `main.rs` with `mimalloc = "0.1"` (default-features off — no extra
features needed). cosmic-text + tiny-skia + syntect do many small allocs
during shaping and rasterization; mimalloc is consistently faster than
glibc's default.

Binary cost: ~500KB (9.1MB → 9.6MB).

### Combined results (1+1b+2+3+4)

n=20 each, fully_rendered = max(first_present, relayout_done):

| doc | baseline | after | savings |
|---|---:|---:|---:|
| README.md | 37.40ms | 26.65–27.90ms | **−9.5 to −10.7ms (−25 to −29%)** |
| examples/test.md | 51.86ms | 44.85–46.01ms | **−5.9 to −7.0ms (−11 to −14%)** |

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
