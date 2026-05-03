pub mod app;
pub mod doc;
pub mod highlight;
pub mod inline;
pub mod layout;
pub mod licenses;
pub mod paint;
pub mod state;
pub mod text;
pub mod theme;
pub mod trace;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cosmic_text::{FontSystem, SwashCache, fontdb};
use winit::event_loop::EventLoop;

use crate::app::{App, SpecResult};
use crate::doc::Doc;
use crate::layout::layout_parallel;
use crate::paint::{Painter, warm_glyph_cache};
use crate::theme::Theme;

/// Number of background FontSystems used for parallel block shaping in
/// `layout_parallel`. With this, layout runs on `1 + N_LAYOUT_WORKERS`
/// lanes (caller thread + workers). Each worker holds its own ~3MB font
/// data; bumping this trades memory for layout speedup. 2 keeps memory
/// modest and gets most of the gain on the typical 30–60-block doc.
const N_LAYOUT_WORKERS: usize = 2;

/// Initial window dimensions (logical px) the speculative layout shapes
/// against, and the viewport extent used for swash glyph cache
/// pre-warming. Must match the value `App::resumed` requests via
/// `Window::default_attributes().with_inner_size(...)` so the
/// speculative result can be reused without a relayout.
const INITIAL_W: f32 = 920.0;
const INITIAL_H: f32 = 1100.0;

pub fn run(source: String, title: String) {
  crate::trace!("run_start");

  // Build worker FontSystems on a background thread so the ~1ms cost
  // overlaps with parse + window/event-loop setup on the main thread.
  let workers_handle = std::thread::spawn(|| -> Vec<FontSystem> {
    (0..N_LAYOUT_WORKERS)
      .map(|_| crate::text::build_font_system())
      .collect()
  });

  let fs = crate::text::build_font_system();
  crate::trace!("fontsystem_ready");

  let doc = crate::doc::parse(&source);
  crate::trace!("doc_parsed");

  let prefs = crate::state::load();
  let dark = prefs.theme.unwrap_or_else(detect_dark);
  let zoom = prefs
    .zoom
    .unwrap_or(1.0)
    .clamp(crate::app::ZOOM_MIN, crate::app::ZOOM_MAX);

  let code_blocks: Vec<(String, String)> = doc
    .blocks
    .iter()
    .filter_map(|b| match b {
      crate::doc::Block::CodeBlock { lang, code } => {
        Some((lang.clone(), code.trim_end_matches('\n').to_string()))
      }
      _ => None,
    })
    .collect();

  // Spawn the syntect precompute thread before building the event loop so
  // its work overlaps with winit/Wayland init. When done, it just sets the
  // atomic — no proxy/wake needed: the redraw loop self-triggers an upgrade
  // after first paint, and `App::relayout` auto-promotes if the flag is
  // already set (saves the entire second pass on no-code/fast-precompute
  // docs like README).
  let highlight_ready = Arc::new(AtomicBool::new(false));
  let ready_for_thread = highlight_ready.clone();
  std::thread::spawn(move || {
    crate::trace!("syntect_warm_start");
    let _ = crate::highlight::syntaxes();
    let _ = crate::highlight::themes();
    crate::trace!("syntect_defaults_ready");
    crate::highlight::precompute(code_blocks, dark);
    crate::trace!("syntect_precompute_done");
    ready_for_thread.store(true, Ordering::Release);
  });

  let layout_workers = workers_handle.join().expect("layout workers build");
  crate::trace!("layout_workers_ready");

  // Speculative layout (item 7): kick off layout on a background thread
  // assuming the window will come up at INITIAL_W × INITIAL_H, dpi=1.0,
  // before we even create the event loop. The thread takes ownership of
  // doc + painter fs + worker fonts + ready flag, runs the same parallel
  // layout the resumed() handler would, and returns everything. The main
  // thread overlaps event-loop / window / surface creation in the
  // meantime. If the actual surface dimensions match the assumption,
  // resumed() reuses the laid doc as-is; otherwise it re-runs layout.
  let assumed_surface_w = INITIAL_W;
  let assumed_dpi_scale = 1.0_f32;
  let assumed_scale = zoom * assumed_dpi_scale.max(1.0);
  let theme = Theme::select(dark);
  let ready_for_layout = highlight_ready.clone();
  let assumed_viewport_h = INITIAL_H * assumed_dpi_scale.max(1.0);
  let layout_handle = std::thread::spawn(
    move || -> SpecResult {
      let mut fs = fs;
      let mut layout_workers = layout_workers;
      let full = ready_for_layout.load(Ordering::Acquire);
      // Spec layout runs in PARALLEL across `1 + N_LAYOUT_WORKERS` lanes.
      // An earlier comment warned that this cost ~2ms in syntect contention
      // pre-T6 (when shaping was ~2× heavier per block). With T6's mono
      // fast-path the parallel window is much smaller and the contention
      // re-measured as a net win. Item (A) on the post-T6 plan.
      let laid = layout_parallel(
        &doc,
        assumed_surface_w,
        &mut fs,
        &mut layout_workers,
        &theme,
        full,
        assumed_scale,
      );
      crate::trace!("speculative_layout_done");
      // Pre-warm the swash glyph cache for the visible viewport while
      // we're still on the bg thread. The painter would otherwise
      // rasterize all these glyphs inline during the first paint
      // (~3-4ms on test.md). The cache_keys we generate reference this
      // fs's font_ids, which match the painter's because all
      // FontSystems load identical fonts in identical order
      // (deterministic slotmap IDs — same reasoning as item 2).
      let mut swash = SwashCache::new();
      warm_glyph_cache(&mut swash, &mut fs, &laid, assumed_viewport_h);
      crate::trace!("speculative_warm_done");
      (doc, fs, layout_workers, laid, full, swash)
    },
  );

  let event_loop = EventLoop::new().expect("event loop");
  crate::trace!("event_loop_created");

  // Defer `layout_handle.join()` to inside `App::resumed` (item T1.5):
  // the join now happens *after* create_window+surface init so bg layout
  // overlaps with Wayland init on the main thread. App is constructed
  // with cheap placeholders for the spec-derived fields; resumed() swaps
  // them in once the join returns.
  let mut app = App {
    wayland_clipboard: None,
    title,
    doc: Doc { blocks: Vec::new() },
    painter: Painter::with_cache(empty_font_system(), SwashCache::new()),
    layout_workers: Vec::new(),
    dark,
    zoom,
    scroll_y: 0.0,
    window: None,
    surface: None,
    pixmap: None,
    laid: None,
    spec_handle: Some(layout_handle),
    speculative_w: assumed_surface_w,
    speculative_scale: assumed_scale,
    painted_once: false,
    full_highlight: false,
    upgrade_pending: false,
    highlight_ready,
    help_visible: false,
    cursor: winit::dpi::PhysicalPosition::new(0.0, 0.0),
    selection: None,
    dragging: false,
    modifiers: Default::default(),
    dpi_scale: 1.0,
    clipboard: None,
  };
  event_loop.run_app(&mut app).expect("run_app");
}

/// Cheap placeholder FontSystem — empty fontdb, no system scan. Used as
/// the painter's initial font system before `resumed()` swaps in the real
/// one returned from the spec thread. `fontdb::Database::new()` is a few
/// µs; this avoids paying for `FontSystem::new()`'s system scan that we
/// deliberately bypass everywhere else.
fn empty_font_system() -> FontSystem {
  FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new())
}

fn detect_dark() -> bool {
  if let Ok(v) = std::env::var("VMD_THEME") {
    return v == "dark";
  }
  true
}
