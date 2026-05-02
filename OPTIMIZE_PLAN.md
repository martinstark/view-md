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

### 5. `opt-level = "s"` is sized for the wrong axis ✅ DONE
**By far the biggest single bang-for-buck change.**

Switched `Cargo.toml` `[profile.release]` `opt-level` from `"s"` to `3`.
Hot loops in `paint.rs` (`blend_pixel`, `pixmap_to_softbuffer`, glyph
rasterization), in cosmic-text shaping during the code-block re-shape,
and in syntect highlighting all benefit from the more aggressive
inlining and vectorization that opt=3 enables.

Binary cost: ~1MB (9.6MB → 10.7MB).

Measured against post-(1+1b+2+3+4) HEAD (n=20 each batch, two batches):

| doc | HEAD (opt=s) | + opt=3 | delta |
|---|---:|---:|---:|
| README.md | 28.27ms | 22.33–22.60ms | **−5.7ms (−20%)** |
| examples/test.md | 45.20ms | 32.76–33.15ms | **−12ms (−27%)** |

Layout itself didn't move much (~12ms in both); the win is in paint
(README, where there's no second pass) and the code-block re-shape
phase (test.md). Suggests cosmic-text shaping is bottlenecked by
something other than instruction throughput, while the per-pixel paint
loops vectorize cleanly under opt=3.

### 6. Speed up per-pixel hot loops in `paint.rs` ✅ DONE
**Saves ~0.5ms in the paint phase (~6–8% of paint).**

Two changes:

1. **Switched glyph rendering from `swash.with_pixels(...)` to
   `swash.get_image(...)`** plus direct iteration over the returned
   bitmap. cosmic-text's `with_pixels` invokes the callback once per
   pixel in the glyph bounding box — including all the alpha=0 pixels,
   which dominate the box for text glyphs. cosmic-text's own docs note
   "use `with_image` for better performance". The new `paint_mask_glyph`
   walks the alpha mask directly with a byte read + branch to skip
   transparent pixels, no closure call.

2. **Hoisted `pixmap.data_mut()` and `pixmap.width()` outside the
   per-pixel hot loop** so `blend_mask_premul` doesn't repeatedly
   re-borrow / re-fetch them.

3. **`pixmap_to_softbuffer` uses `chunks_exact(4)`** to give the
   optimizer bounds-elision and vectorization info on the ~1M-pixel
   BGRA→u32 conversion per frame.

Measured (n=50 each):

| metric | HEAD (onig) | + paint opts | delta |
|---|---:|---:|---:|
| test.md paint_dur (fp − redraw_first) | 5.61ms | 5.18ms | **−0.43ms (−8%)** |
| README.md paint_dur | 8.10ms | 7.62ms | **−0.48ms (−6%)** |

Honest framing: an earlier attempt (chunks_exact + pre-extraction
only, no `get_image` switch) bought ~0.3ms and was within noise. The
`get_image` switch is what actually moved the needle, and it only
moved it ~0.5ms because paint is no longer the dominant phase
post-onig — `pixmap.fill`, swash glyph rasterization, and Wayland
present each contribute their own fixed costs.

### 7. Start window/surface creation in parallel with layout ✅ DONE
**Saves ~1.5ms on first_present everywhere; ~1.7ms on fully_rendered for no-code docs.**

After parse, a background thread takes ownership of the doc + painter
FontSystem + worker FontSystems and runs `layout_parallel` against an
assumed surface (`INITIAL_W = 920`, dpi_scale = 1.0). The main thread
overlaps event-loop creation in the meantime; on `resumed()` we
compare the actual surface dimensions/scale against the assumption and
reuse the laid doc when they match (the common case on this system).

**Subtlety**: the speculative layout runs *sequentially* (workers slice
passed empty), not in parallel. Reason: syntect precompute fans out
to one OS thread per code block, and on a 16-thread CPU adding 2
worker threads from a parallel speculative layout cost ~2ms in
syntect_precompute_done due to scheduler contention — wiping out the
fully_rendered win on code-heavy docs. Sequential takes ~3ms longer
inside the bg thread but stays off the critical path because main is
doing 3–4ms of event-loop work in the meantime.

Measured against post-(5) HEAD (n=20 each):

| doc | HEAD (opt-3) | + speculative seq |
|---|---:|---:|
| README.md fully_rendered | 22.33–22.60ms | **20.82ms (−1.7ms)** |
| examples/test.md first_present | 21.03–21.57ms | **19.76ms (−1.5ms)** |
| examples/test.md fully_rendered | 32.76–33.15ms | 32.77ms (flat — syntect-bound) |

For code-heavy docs the user sees first paint sooner but the
"highlighted" finish happens at the same wall-clock moment because
syntect precompute is the bottleneck. The fully_rendered ceiling
won't drop further without parallelizing or batching the per-block
syntect work itself.

### 7b. Skip the request_redraw scheduling round-trip for first paint ✅ DONE
**Saves ~1.5–3ms on first_present.**

`App::resumed` previously called `window.request_redraw()` and waited
for winit to fire `WindowEvent::RedrawRequested` before painting. That
took ~1.5–2.3ms of pure scheduling delay on Wayland. New version just
calls `self.redraw()` synchronously at the end of `resumed()`. The
upgrade pass still flows through the normal event loop (the redraw
function calls `request_redraw` itself if the upgrade isn't ready
yet), so the runtime semantics are identical — only the first paint
short-circuits the schedule.

Plus a defensive in-line check in `redraw()`: if `highlight_ready` is
already true at redraw entry (and we haven't promoted yet), do the
in-place upgrade BEFORE painting. This collapses the
placeholder-then-highlighted flash into a single highlighted paint
when syntect happens to finish before we redraw — common on no-code
docs.

Measured (n=50 each):

| metric | HEAD | + sync paint + ready check |
|---|---:|---:|
| test.md fp mean | 16.28ms | 14.68ms |
| test.md fp min | 12.28ms | **10.64ms** |
| test.md fully_rendered | 16.70ms | 16.52ms |
| test.md fully min | 14.79ms | **12.85ms** |
| README.md fp mean | 17.28ms | 16.03ms |
| README.md fp min | 14.78ms | **12.58ms** |
| README.md fully_rendered | 17.28ms | 16.03ms (= fp; single paint) |

Best-case minimums are now ~10–13ms — within striking distance of
the 8.3ms one-frame-at-120Hz target on individual runs but not yet
consistently. The cost is primarily winit/Wayland init (~5ms before
we even paint) and swash glyph rasterization (~3–4ms), neither of
which we can easily eliminate.

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
- ✅ **`syntect default-fancy` → `default-onig`**: switched. The previous
  description of this as "saves a bit of cold-start" was wildly wrong —
  this turned out to be the single biggest syntect lever.

  syntect 5.3 doesn't ship with the Rust `regex` crate as a backend
  option; the choice is between `regex-fancy` (fancy-regex Rust crate)
  and `regex-onig` (the Oniguruma C library, which is what the bundled
  TextMate-style grammars were originally targeted at). Onig is
  dramatically faster on these grammars in practice.

  Measured (n=20 each) against post-(7) HEAD:

  | doc | HEAD (fancy) | + onig | delta |
  |---|---:|---:|---:|
  | examples/test.md syntect_done | 30.73ms | 8.17ms | **−22.6ms (−74%)** |
  | examples/test.md fully_rendered | 32.77ms | 19.74–20.62ms | **−12 to −13ms (−37–40%)** |
  | README.md (no code) | 20.82ms | 20.81–21.17ms | unchanged |
  | NATIVE_MD_VIEW.md | not measured | 33.54ms | n/a |

  Cost: adds a C dependency (libonig built from the `onig_sys` crate).
  Build environment now needs a working C compiler. Binary size
  basically unchanged (~9.6MB).
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
