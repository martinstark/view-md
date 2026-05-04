#![allow(clippy::too_many_arguments)]

pub mod app;
pub mod doc;
pub mod highlight;
pub mod images;
pub mod inline;
pub mod layout;
pub mod licenses;
pub mod paint;
pub mod state;
pub mod text;
pub mod theme;
pub mod trace;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cosmic_text::{FontSystem, SwashCache, fontdb};
use winit::event_loop::EventLoop;

use crate::app::{App, SpecResult};
use crate::doc::Doc;
use crate::layout::layout_parallel;
use crate::paint::{Painter, warm_glyph_cache_parallel};
use crate::theme::Theme;

/// User events the winit event loop dispatches to `App::user_event`. The
/// loop is always typed against this enum even when no events are sent
/// (i.e. when not watching), so the dispatch path is identical and
/// adding a watcher later doesn't change the loop type.
#[derive(Debug, Clone)]
pub enum AppEvent {
  /// A file the user passed `--watch` for has changed on disk; re-read,
  /// reparse, relayout, redraw.
  Reload,
  /// One of the inline images finished decoding on the bg thread; just
  /// request a redraw — dimensions were known at parse time so layout
  /// is unchanged.
  ImageReady,
}

/// Number of background FontSystems used for parallel block shaping in
/// `layout_parallel`. With this, layout runs on `1 + N_LAYOUT_WORKERS`
/// lanes (caller thread + workers). Each worker holds its own ~3MB font
/// data; bumping this trades memory for layout speedup. 2 keeps memory
/// modest and gets most of the gain on the typical 30–60-block doc.
const N_LAYOUT_WORKERS: usize = 2;

/// Default window dimensions (logical px) used on first run when no
/// saved size is in prefs. Once the user resizes and exits, the saved
/// size in `~/.local/state/vmd/prefs` overrides these.
///
/// The chosen size is also what the speculative layout shapes against
/// and what `App::resumed` requests via
/// `Window::default_attributes().with_inner_size(...)`. As long as the
/// two stay in sync (via `App.initial_logical_w/h`), the speculative
/// result is reused without a relayout.
pub const DEFAULT_W: f32 = 920.0;
pub const DEFAULT_H: f32 = 1100.0;

pub fn run(
  source: String,
  title: String,
  watch_path: Option<PathBuf>,
  base_dir: Option<PathBuf>,
  anchor: Option<String>,
) {
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

  // Image dimensions need to be known before layout so that block-image
  // boxes get the right size and there's no layout shift when pixels
  // arrive from the bg decoder. Header-only reads are µs even for
  // multi-MB sources, so doing this on the main thread before kicking
  // off layout is fine; full pixel decode runs asynchronously below.
  let images = Arc::new(crate::images::ImageStore::new());
  let image_paths = crate::images::collect_image_paths(&doc, base_dir.as_deref());
  for p in &image_paths {
    let dims = crate::images::read_dims(p);
    images.insert_dims(p.clone(), dims);
  }
  crate::trace!("image_dims_read n={}", image_paths.len());

  let prefs = crate::state::load();
  let dark = prefs.theme.unwrap_or_else(detect_dark);
  let zoom = prefs
    .zoom
    .unwrap_or(1.0)
    .clamp(crate::app::ZOOM_MIN, crate::app::ZOOM_MAX);
  let initial_logical_w = prefs.width.unwrap_or(DEFAULT_W);
  let initial_logical_h = prefs.height.unwrap_or(DEFAULT_H);

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
  let assumed_surface_w = initial_logical_w;
  let assumed_dpi_scale = 1.0_f32;
  let assumed_scale = zoom * assumed_dpi_scale.max(1.0);
  let theme = Theme::select(dark);
  let ready_for_layout = highlight_ready.clone();
  let assumed_viewport_h = initial_logical_h * assumed_dpi_scale.max(1.0);
  let images_for_spec = images.clone();
  let base_dir_for_spec = base_dir.clone();
  let layout_handle = std::thread::spawn(move || -> SpecResult {
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
      images_for_spec,
      base_dir_for_spec,
    );
    crate::trace!("speculative_layout_done");
    // Pre-warm the swash glyph cache for the visible viewport in
    // PARALLEL across `1 + N_LAYOUT_WORKERS` lanes (item B). Each lane
    // uses its own SwashCache; after all done, worker caches drain
    // into the main cache the painter will own. cache_key compatibility
    // across FontSystems is the same property layout_parallel relies
    // on (deterministic fontdb slotmap IDs).
    let mut swash = SwashCache::new();
    let mut worker_swashes: Vec<SwashCache> = (0..layout_workers.len())
      .map(|_| SwashCache::new())
      .collect();
    warm_glyph_cache_parallel(
      &mut swash,
      &mut fs,
      &mut layout_workers,
      &mut worker_swashes,
      &laid,
      assumed_viewport_h,
    );
    crate::trace!("speculative_warm_done");
    (doc, fs, layout_workers, laid, full, swash)
  });

  let event_loop = EventLoop::<AppEvent>::with_user_event()
    .build()
    .expect("event loop");
  crate::trace!("event_loop_created");

  if let Some(path) = watch_path.clone() {
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || spawn_watcher(path, proxy));
    crate::trace!("watcher_spawned");
  }

  if !image_paths.is_empty() {
    let proxy = event_loop.create_proxy();
    let store = images.clone();
    let paths = image_paths.clone();
    std::thread::spawn(move || decode_images(paths, store, proxy));
  }

  // Defer `layout_handle.join()` to inside `App::resumed` (item T1.5):
  // the join now happens *after* create_window+surface init so bg layout
  // overlaps with Wayland init on the main thread. App is constructed
  // with cheap placeholders for the spec-derived fields; resumed() swaps
  // them in once the join returns.
  let mut app = App {
    wayland_clipboard: None,
    title,
    watch_path,
    base_dir,
    pending_anchor: anchor,
    images: images.clone(),
    doc: Doc { blocks: Vec::new() },
    painter: Painter::with_cache(empty_font_system(), SwashCache::new()),
    layout_workers: Vec::new(),
    dark,
    zoom,
    scroll_y: 0.0,
    window: None,
    surface: None,
    surface_size: (0, 0),
    laid: None,
    spec_handle: Some(layout_handle),
    speculative_w: assumed_surface_w,
    speculative_scale: assumed_scale,
    initial_logical_w,
    initial_logical_h,
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
    anim_start: std::time::Instant::now(),
    anim_next_deadline: None,
    empty_reload_deadline: None,
    search: None,
    hint: None,
    toast: None,
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

/// Decode each image's pixels in document order so the above-the-fold
/// image gets a redraw first. Each decode posts `AppEvent::ImageReady`,
/// which `App::user_event` turns into a redraw — no relayout, since
/// dimensions were locked in synchronously at parse time.
fn decode_images(
  paths: Vec<PathBuf>,
  store: Arc<crate::images::ImageStore>,
  proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) {
  for path in paths {
    if store.get_frames(&path).is_some() {
      continue;
    }
    crate::trace!("image_decode_start {}", path.display());
    let mut count = 0u32;
    let path_for_cb = path.clone();
    let store_for_cb = store.clone();
    let proxy_for_cb = proxy.clone();
    let ok = crate::images::decode_streaming(&path, move |f| {
      store_for_cb.append_frame(&path_for_cb, f);
      count += 1;
      if count == 1 {
        crate::trace!("image_first_frame {}", path_for_cb.display());
      }
      // Coalescing happens in winit: many request_redraw calls collapse
      // to one RedrawRequested event per frame, so per-frame posting is
      // cheap even on a 1000-frame webp.
      let _ = proxy_for_cb.send_event(AppEvent::ImageReady);
    });
    if ok {
      crate::trace!("image_decode_done {}", path.display());
    } else {
      store.set_failed(&path);
      crate::trace!("image_decode_failed {}", path.display());
      let _ = proxy.send_event(AppEvent::ImageReady);
    }
  }
}

/// Watches the parent directory of `path` (so editor rename-replace saves
/// are seen) and posts `AppEvent::Reload` whenever an event touches the
/// target file. Coalescing/debouncing is left to the user_event handler:
/// re-reading a stable file twice is microseconds and idempotent.
fn spawn_watcher(path: PathBuf, proxy: winit::event_loop::EventLoopProxy<AppEvent>) {
  use notify::{Config, EventKind, RecursiveMode, Watcher};
  let dir = path
    .parent()
    .map(|p| p.to_path_buf())
    .unwrap_or_else(|| PathBuf::from("."));
  let target = path.canonicalize().unwrap_or(path);
  let proxy_clone = proxy.clone();
  let target_clone = target.clone();
  let mut watcher = match notify::RecommendedWatcher::new(
    move |res: notify::Result<notify::Event>| {
      let Ok(ev) = res else { return };
      if !matches!(
        ev.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
      ) {
        return;
      }
      let touches = ev
        .paths
        .iter()
        .any(|p| p == &target_clone || p.canonicalize().ok().as_deref() == Some(&target_clone));
      if touches {
        crate::trace!("watcher_event {:?} {:?}", ev.kind, ev.paths);
        let _ = proxy_clone.send_event(AppEvent::Reload);
      }
    },
    Config::default(),
  ) {
    Ok(w) => w,
    Err(e) => {
      eprintln!("vmd: watcher init failed: {e}");
      return;
    }
  };
  if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
    eprintln!("vmd: watch {}: {e}", dir.display());
    return;
  }
  // Block forever; the watcher delivers events via the closure above.
  std::thread::park();
  drop(watcher);
}
