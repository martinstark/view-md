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

use cosmic_text::FontSystem;
use winit::event_loop::EventLoop;

use crate::app::App;
use crate::paint::Painter;

/// Number of background FontSystems used for parallel block shaping in
/// `layout_parallel`. With this, layout runs on `1 + N_LAYOUT_WORKERS`
/// lanes (caller thread + workers). Each worker holds its own ~3MB font
/// data; bumping this trades memory for layout speedup. 2 keeps memory
/// modest and gets most of the gain on the typical 30–60-block doc.
const N_LAYOUT_WORKERS: usize = 2;

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

  let event_loop = EventLoop::new().expect("event loop");
  crate::trace!("event_loop_created");

  let layout_workers = workers_handle.join().expect("layout workers build");
  crate::trace!("layout_workers_ready");

  let mut app = App {
    wayland_clipboard: None,
    title,
    doc,
    painter: Painter::new(fs),
    layout_workers,
    dark,
    zoom,
    scroll_y: 0.0,
    window: None,
    surface: None,
    pixmap: None,
    laid: None,
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

fn detect_dark() -> bool {
  if let Ok(v) = std::env::var("VMD_THEME") {
    return v == "dark";
  }
  true
}
