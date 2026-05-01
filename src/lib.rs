pub mod app;
pub mod doc;
pub mod highlight;
pub mod inline;
pub mod layout;
pub mod licenses;
pub mod paint;
pub mod text;
pub mod theme;
pub mod trace;

use winit::event_loop::EventLoop;

use crate::app::App;
use crate::paint::Painter;

pub fn run(source: String, title: String) {
    crate::trace!("run_start");

    // Warm syntect off the critical path. SyntaxSet::load_defaults_newlines
    // unpacks ~38ms of bincode; we want it ready by the time layout hits a
    // code block, but not blocking window creation.
    std::thread::spawn(|| {
        crate::trace!("syntect_warm_start");
        let _ = crate::highlight::syntaxes();
        crate::trace!("syntect_syntaxes_ready");
        let _ = crate::highlight::themes();
        crate::trace!("syntect_themes_ready");
    });

    let fs = crate::text::build_font_system();
    crate::trace!("fontsystem_ready");

    let doc = crate::doc::parse(&source);
    crate::trace!("doc_parsed");

    let event_loop = EventLoop::new().expect("event loop");
    crate::trace!("event_loop_created");

    let mut app = App {
        title,
        doc,
        painter: Painter::new(fs),
        dark: detect_dark(),
        scroll_y: 0.0,
        window: None,
        surface: None,
        pixmap: None,
        laid: None,
        painted_once: false,
        full_highlight: false,
        upgrade_pending: false,
    };
    event_loop.run_app(&mut app).expect("run_app");
}

fn detect_dark() -> bool {
    if let Ok(v) = std::env::var("MDV_THEME") {
        return v == "dark";
    }
    true
}
