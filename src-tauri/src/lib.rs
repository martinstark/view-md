mod render;

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde::Serialize;
use tauri::Manager;

static APP_START: OnceLock<Instant> = OnceLock::new();

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocPayload {
    pub html: String,
    pub title: String,
    pub css_light: String,
    pub css_dark: String,
}

struct DocCell(Arc<OnceLock<DocPayload>>);
struct DocTitle(String);

fn trace(label: &str) {
    if std::env::var_os("MDV_TRACE").is_some() {
        let elapsed = APP_START
            .get()
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);
        eprintln!("[mdv] {elapsed:>5}ms {label}");
    }
}

#[tauri::command]
fn load_document(state: tauri::State<DocCell>) -> DocPayload {
    trace("load_document_called");
    loop {
        if let Some(p) = state.0.get() {
            trace("load_document_returned");
            return p.clone();
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

pub fn run(source: String, title: String) {
    let _ = APP_START.set(Instant::now());
    trace("run_start");

    let cell: Arc<OnceLock<DocPayload>> = Arc::new(OnceLock::new());
    let cell_w = cell.clone();
    let title_for_render = title.clone();

    std::thread::spawn(move || {
        trace("render_start");
        let html = render::render(&source);
        trace("render_done");
        let (css_light, css_dark) = render::theme_css();
        trace("theme_css_done");
        let _ = cell_w.set(DocPayload {
            html,
            title: title_for_render,
            css_light,
            css_dark,
        });
        trace("payload_ready");
    });

    trace("tauri_builder_start");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(DocCell(cell))
        .manage(DocTitle(title))
        .invoke_handler(tauri::generate_handler![load_document])
        .setup(|app| {
            trace("setup");
            if let Some(win) = app.get_webview_window("main") {
                let t = app.state::<DocTitle>().0.clone();
                let _ = win.set_title(&format!("{t} — mdv"));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
