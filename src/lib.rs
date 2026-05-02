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

use winit::event_loop::EventLoop;

use crate::app::App;
use crate::paint::Painter;

pub fn run(source: String, title: String) {
  crate::trace!("run_start");

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

  // Spawn the syntect precompute as early as possible, before
  // EventLoop::new() and window/surface setup. The thread runs in
  // parallel with the entire setup-to-first-paint path; on this hardware
  // all 3 blocks finish around the time first_present fires, so the
  // frame-2 relayout finds spans already cached.
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
  std::thread::spawn(move || {
    crate::trace!("syntect_warm_start");
    let _ = crate::highlight::syntaxes();
    let _ = crate::highlight::themes();
    crate::trace!("syntect_defaults_ready");
    crate::highlight::precompute(code_blocks, dark);
    crate::trace!("syntect_precompute_done");
  });

  let event_loop = EventLoop::new().expect("event loop");
  crate::trace!("event_loop_created");

  let mut app = App {
    wayland_clipboard: None,
    title,
    doc,
    painter: Painter::new(fs),
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
