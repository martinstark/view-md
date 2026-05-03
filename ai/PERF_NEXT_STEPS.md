# vmd launch-time deep dive — every thread to pull for sub-8.3ms

## Where we stand

Per `OPTIMIZE_PLAN.md`, post-(1+1b+2+3+4+5+6+7+7b+7c+7e), best-case minimums are:

- **README.md `fp_min`: 8.78ms** — already 0.5ms over budget, occasional sub-8.3ms runs
- **test.md `fp_min`: 10.31ms** — 2ms over budget consistently
- **Means ~13–14ms** — ~5–6ms over budget

So the goal is shaving **~2ms off the mean** and tightening variance so the min becomes the typical case. Below I rank everything left, including ideas not yet considered.

---

## Critical-path anatomy at `t=0` → `first_present`

```
[main]   t=0     start, alloc init, mimalloc warm
[bg-w]   spawn workers thread (build N FontSystems)
[main]   build_font_system          ~0.09ms      lib.rs:51
[main]   doc::parse                 ~0.6ms       lib.rs:54
[main]   state::load (XDG file rd)  ~0.2-0.5ms   lib.rs:57
[bg-s]   spawn syntect precompute   (~8ms onig, off path)
[main]   workers_handle.join        wait ~0
[bg-l]   spawn layout_handle (spec layout + warm) ~6ms
[main]   EventLoop::new             ~1.5-2.3ms   lib.rs:147
[main]   layout_handle.join         wait if bg slower
[main]   App {} construct           ~0
[main]   event_loop.run_app         → resumed()
[main]   resumed: create_window     ~1ms         app.rs:125
[main]   smithay_clipboard::new     ~0.1ms       app.rs:137
[main]   softbuffer Context+Surface ~0.3ms       app.rs:144-145
[main]   surface.resize             ~0.1ms
[main]   Pixmap::new + zero         ~0.5ms       app.rs:153
[main]   speculative dims check     ~0
[main]   syntect 5ms-cap busy-wait  0–5ms        app.rs:179-188
[main]   redraw():
           paint_doc (warm cache)   ~1.85ms      paint.rs:26
           pixmap_to_softbuffer     ~1ms         paint.rs:848
           buffer.present()         ~0.5-1ms (Wayland commit)
[main]   first_present              ~10–14ms
```

The **irreducible Wayland/winit floor** (EventLoop + create_window + surface + present) is ~5ms — that's the wall the rest of the optimizations are pressed against.

---

## Threads to pull (ranked by EV)

### Tier 1 — high-EV, moderate effort

#### T1. Pre-render the first pixmap on the spec-layout thread
**Expected save: 1.5–2.0ms on first_present, every doc.**

The spec thread (`lib.rs:110-145`) currently does layout + glyph-cache warm and returns a `LaidDoc + SwashCache`. It has everything it needs to *also* paint into a pixmap. Move all of `paint_doc` to the bg thread:

```rust
// in layout_handle:
let mut pre_pixmap = Pixmap::new(W, H).unwrap();
let painter_tmp = Painter::with_cache(fs, swash);
painter_tmp.paint_doc(&mut pre_pixmap, &laid, &theme, 0.0);
let (fs, swash) = (painter_tmp.fs, painter_tmp.swash);
return (..., laid, swash, pre_pixmap);
```

In `resumed()`, when dims match (the common case), substitute `pre_pixmap` for the freshly-allocated one and skip the `redraw()` paint phase — go straight to `pixmap_to_softbuffer + present`.

Critical-path math: paint_doc with warm cache is ~1.85ms (per OPTIMIZE_PLAN 7c). That comes off the main thread entirely. Bg thread length goes from ~6ms → ~7.8ms; main thread already idles ~4.6ms waiting for the join, so most of this hides under existing slack.

#### T2. Pre-convert pixmap → u32 buffer on bg thread
**Expected save: 0.5–1.0ms.** Builds on T1.

`pixmap_to_softbuffer` is a 1MP×4-byte loop (~1ms). Run it on the spec thread into a `Vec<u32>`. In `resumed()`:

```rust
let mut buffer = surface.buffer_mut()?;
buffer.copy_from_slice(&prerendered);  // ~0.3ms memcpy
buffer.present()?;
```

Caveat: softbuffer's buffer is mmap'd from wl_shm. `copy_from_slice` is a single `memcpy` to that mapped region — ~3× faster than the `iter().zip(chunks_exact(4))` conversion since the conversion is now done on bg.

#### T3. Move doc::parse + state::load off main thread
**Expected save: 0.5–1.0ms.**

Both run synchronously on main between `build_font_system` and the spec layout spawn (lib.rs:54, 57). They're each ~0.3–0.6ms and main thread is single-threaded waiting on them.

Move them to the existing workers thread (or a new `parse_thread`). Layout handle waits for parse, but parse runs in parallel with `EventLoop::new()`. New main timeline:

```
t=0 spawn parse_thread + workers_thread
t=0 EventLoop::new (~2ms)        ← starts immediately, no longer waits for parse
t=2 join parse + workers
t=2 spawn layout_handle (does parse + spec + warm + paint)
```

The EventLoop call moves from `t≈1` to `t=0`, freeing ~1ms on the bound path.

#### T4. Eliminate the up-to-5ms `syntect_wait` on code-heavy docs
**Expected save: 0–4ms (variable, code-heavy docs).** Hard tradeoff with the placeholder→highlighted flash.

Two subvariants:

- **T4a.** Move the upgrade *into the spec thread*: spec thread layouts placeholder, warms, then if syntect is ready *or once it becomes ready (with a tighter 2ms cap)* run `upgrade_code_block_highlights` and re-warm just the upgraded code blocks. Eliminates the wait from main thread entirely and removes the flash deterministically — the spec thread already has the syntect handle visible (`ready_for_layout`).
- **T4b.** Pre-compile syntect on a thread spawned *at static init* (before `main()` even runs the trace), via a `ctor`-style hook. Removes ~5ms of syntect bootstrap from the program-start clock entirely. Less portable.

Strong recommendation for T4a: spec thread is the right home for this work.

### Tier 2 — high impact, larger lift

#### T5. Drop winit, use sctk (smithay-client-toolkit) directly
**Expected save: 1.5–3ms.** Largest remaining mechanical win.

winit 0.30 abstracts the configure-roundtrip and forces work into `resumed()`. With sctk:

- **Connect to Wayland on a bg thread** (winit forces main thread); the connection roundtrip overlaps with parse + workers spawn.
- **Send `wl_surface.create + xdg_toplevel + commit` empty** while spec layout runs; configure ack arrives by the time you're ready to paint.
- **Attach + commit the pre-rendered buffer in one batch** — eliminates ~1ms of winit's per-call wl_callback flushing.

Expected end-state: critical path becomes ~3.5–5ms exec→present on this hardware. Only meaningful path to consistent <8.3ms.

Cost: ~2–3 day refactor; loses X11/macOS support unless you keep a winit feature path. Given the personal-use Wayland-only scope, acceptable.

#### T6. Bypass cosmic-text shaping for monospace code blocks
**Expected save: 1.5–3ms on code-heavy docs.**

`build_highlighted_buffer` (layout.rs:606) runs `Shaping::Advanced` per code block. JBM is fixed-pitch — we don't need the BiDi/segmentation/break-iteration machinery. Build a thin `MonoBuffer` that:
- Looks up glyph_id once per unique char
- Places glyphs at `(col_index * advance, row_index * line_height)`
- Reuses cosmic-text's `LayoutRun` shape just enough for hit-testing + selection paths

This is the same insight tofi exploits with HarfBuzz on a known string — but you don't need HarfBuzz at all because there's no shaping to do.

Quality cost: lose `→` `≠` `≥` ligatures in code (the OPTIMIZE_PLAN already considered this and rejected it — but at ~2ms it might now be worth it). Could keep ligatures by hard-coding a small ligature table (`->`, `=>`, `!=`, `==`, `>=`, `<=`, `...`, `::`) — 8 lookups per char-pair, still 5× faster than full shaping.

#### T7. Pre-allocate Pixmap on bg thread
**Expected save: 0.3–0.5ms.** Trivial.

`Pixmap::new(w, h)` allocates ~4MB and zeroes it. Currently in `resumed()` (app.rs:153). Move to spec thread (along with T1).

### Tier 3 — small, easy

#### T8. Drop `Inter-BoldItalic` and `JetBrains Mono Italic`
**Expected save: ~50–100µs per FontSystem build × 3 systems = 200–300µs.**

These are rarely hit and add ~700KB of ttf data each FontSystem must parse. The OPTIMIZE_PLAN flags them as candidates.

#### T9. Skip `state::load()` on first paint, apply prefs after
**Expected save: 0.2–0.5ms.**

Disk I/O on critical path. Default to env-var or `dark`, kick off a bg load, apply once present (causes a relayout if zoom/theme differ — flash risk).

For zoom-default users (the common case), this is a free win.

#### T10. Remove the `Arc::clone` + `Mutex<HashMap>` syntect cache for warm paths
**Expected save: 0.1ms (cache lookup contention).**

`highlight()` (highlight.rs:40) acquires a `Mutex<HashMap>` lock on every code-block lookup. For test.md (12 code blocks) that's 24 lock ops on first paint. Switch to `OnceLock<DashMap>` or per-spec-thread thread-local caches with merge-back.

#### T11. Limit syntect precompute concurrency
**Expected save: 0.5–1ms in the warm thread; helps T4a determinism.**

`highlight::precompute` (highlight.rs:137) spawns one OS thread *per code block*. test.md → 12 threads + 2 layout workers + spec thread ≈ 15 threads on a 16-core. Scheduler thrashing is mentioned in the lib.rs comment ("syntect precompute thread fans out to one OS thread per code block on a 16-thread CPU"). Use a bounded thread pool (~4 threads) and reuse them.

#### T12. `pixmap_to_softbuffer` SIMD or memcpy via swizzle
**Expected save: 0.3–0.5ms.**

The current loop processes 4 BGRA bytes → `u32` per iteration. `unsafe { transmute }` of 4 byte chunks to `u32` + a single byteswap (`v.rotate_left(8) & 0xFFFFFF` style) is hot-loop-friendly. With opt=3 the compiler may already vectorize, but verify with `cargo asm` — there's room for a packed `_mm_shuffle_epi8` lane that's strictly faster than chunks_exact.

Even better: paint directly into the `&mut [u32]` softbuffer slice from the get-go — skip tiny-skia's BGRA pixmap step. Requires teaching `paint_doc` and `paint_block` to write u32. Larger refactor (~1 day) but kills a full pass over 1MP.

#### T13. Don't `surface.resize()` if dims already match
**Expected save: <0.1ms.** Defensive code.

#### T14. Fold `self.window = ...` and `self.surface = ...` writes earlier
**Expected save: <0.1ms.** Cosmetic; currently between syntect-wait and redraw.

### Tier 4 — speculative / unconventional

#### T15. memfd_create + transparent hugepages for the wl_shm buffer (NATIVE_MD_VIEW.md A.4)
**Expected save: 0.5–2ms first-paint page-fault cost.**

softbuffer wraps wl_shm. First-paint touches every 4-byte cell of the buffer, faulting in pages. For 4MB at 4KB pages = 1024 page faults. With THP-shmem advise mode, that's 2 page faults instead. Fork the buffer alloc to use `memfd_create` directly (or PR softbuffer).

#### T16. Cache the LaidDoc to disk
**Expected save: 5–8ms for repeat opens of same file.** OPTIMIZE_PLAN item 8.

Key by `(path, mtime, theme, zoom, dpi, surface_w)`. Layout becomes ~1ms (deserialize) instead of ~3ms. cosmic-text Buffers aren't trivially serializable, but glyph positions + layout runs are. Big complexity.

#### T17. Pre-render at static-init time with `ctor`
**Expected save: 2–3ms apparent.**

Use `[[ctor]]` (or inline asm) to run heavy work *before* `main()`'s trace clock starts. Move FontSystem build, spawn syntect, even spawn the spec layout thread — all before `main()` runs. The trace clock starts later, so the *measured* startup looks shorter even though wall-clock is the same. Cheap instrumentation gain only — but if "8.3ms" is measured from your trace marker, this hits the target trivially.

(Honest caveat: it doesn't actually make pixels appear sooner. Only changes what you're measuring.)

#### T18. Smaller initial window size
**Expected save: 0.5–1ms (less to fill, less to paint, smaller buffer to send).**

INITIAL_W=920, INITIAL_H=1100 → ~1MP. Drop to 800×900 → ~720KP, 28% less. UX cost: smaller default window. Probably not worth it.

#### T19. Skip the syntect *precompute* on the no-code-block path entirely
**Expected save: 0–8ms scheduler/CPU pressure.**

Currently the syntect spawn always runs and always touches `ThemeSet::load_defaults` etc. For docs with zero code blocks (README), the work is wasted but still steals CPU from the main thread's runtime work. Gate the spawn on `code_blocks.is_empty()`.

#### T20. Static-init the SyntaxSet
**Expected save: 1–2ms cold start.**

`SyntaxSet::load_defaults_newlines()` deserializes a bincoded blob. Pre-bake it as a const Rust struct via `build.rs` (or use `lazy_static` with `lazy_load = false` semantics). Eliminates the deserialization pass, but the regex compilation still happens lazily. Modest win.

---

## Recommended sequence to hit <8.3ms consistently

1. **T1 + T7 + T2 (pre-render pixmap on bg thread)** — first attack, shave ~2ms. Should put min ≈6.5–7ms, mean ≈11–12ms.
2. **T3 (parse + state on bg)** — another ~0.5ms off the mean.
3. **T4a (eliminate syntect wait on critical path)** — 0–4ms off code-heavy docs; eliminates variance.
4. **T11 (bounded syntect threadpool)** — tightens variance further.
5. **T6 (mono fast path)** — only if (1)–(4) leave you above target on test.md.
6. **T5 (sctk-direct)** — last lever needed if anything remains. Largest single win, largest cost.

After (1)–(4), I'd expect mean around 7–9ms and min around 5–6ms — 8.3ms-target hit-rate ~70%+. Adding (5) gets it to >95%.

## What I'd skip

- **T8 / T13 / T14** — sub-100µs each. Noise-level.
- **T15 (memfd hugepages)** — high friction, mostly cosmetic on this hardware.
- **T16 (LaidDoc cache)** — major engineering for a niche scenario (same file repeat-open).
- **T17 (ctor cheating)** — gaming the metric, not improving the experience.
- **T20 (bincode → const)** — small win, large code-gen cost.

## Things to measure before committing

- The actual cost of `pixmap.fill` vs. `paint_doc` body: `paint_doc` already calls `pixmap.fill` first (paint.rs:27). The 1.85ms post-prewarm number is fill+block-iter+blends combined. T1 must absorb all of it onto the bg thread to actually save the full amount.
- Whether `EventLoop::new()` actually scales with main-thread idle time on Wayland (it has a wl_display.roundtrip, so yes, but worth confirming).
- Whether `surface.buffer_mut()` after T2 still incurs the wl_shm mmap cost (it does — that's not movable; but the cost is small ~0.1ms).

The single most valuable next step is **T1 + T2 + T7 together as one PR** — it's the largest mechanical win that doesn't require dropping winit, and the architecture supports it cleanly (`Painter` already takes a `SwashCache` via `with_cache`; adding a `Pixmap` follows the same pattern).
