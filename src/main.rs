use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use vmd::trace;

// cosmic-text + tiny-skia + syntect do many small allocs during shaping
// and rasterization. mimalloc is consistently faster than glibc malloc on
// these workloads and adds ~150KB to the binary.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn usage() -> ! {
  eprintln!(
    "vmd — fast native viewer for markdown and JSON\n\
         \n\
         usage: vmd [flags] <file[.md|.json|.jsonc|.json5][#anchor] | ->\n\
         \n\
         flags:\n\
         \x20\x20--watch        reload the file on disk changes\n\
         \x20\x20--json         force JSON mode (skip extension/content sniff)\n\
         \x20\x20--licenses     print bundled font licenses (SIL OFL 1.1)\n\
         \x20\x20--trace        print timing breakdown (also: VMD_TRACE=1)\n\
         \x20\x20-h, --help     this message\n\
         \n\
         keybinds (press ? in the app for the full list):\n\
         \x20\x20q / Esc        quit\n\
         \x20\x20t              toggle theme\n\
         \x20\x20j k d u f b    scroll line / half / full page\n\
         \x20\x20g G            top / bottom\n\
         \x20\x20] [ }} {{      next/prev heading / block\n\
         \x20\x20/              search; Enter cycles matches, Esc closes\n\
         \x20\x20+ - 0          zoom in / out / reset\n\
         \x20\x20y              yank visible code block\n\
         \x20\x20Ctrl+C         copy selected text"
  );
  std::process::exit(2);
}

fn main() -> ExitCode {
  trace::init();
  crate::trace!("main");

  let mut path: Option<String> = None;
  let mut from_stdin = false;
  let mut watch = false;
  let mut force_json = false;
  for arg in std::env::args().skip(1) {
    match arg.as_str() {
      "--licenses" => {
        vmd::licenses::print();
        return ExitCode::SUCCESS;
      }
      "--trace" => {
        trace::enable();
      }
      "--watch" => {
        watch = true;
      }
      "--json" => {
        force_json = true;
      }
      "-h" | "--help" => usage(),
      "-" => from_stdin = true,
      s if s.starts_with("--") => {
        eprintln!("vmd: unknown flag: {s}");
        return ExitCode::from(2);
      }
      s => {
        if path.is_some() {
          usage();
        }
        path = Some(s.to_string());
      }
    }
  }

  if watch && from_stdin {
    eprintln!("vmd: --watch requires a file path, not stdin");
    return ExitCode::from(2);
  }

  let (source, title, watch_path, base_dir, anchor, ext_is_json) = if from_stdin {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
      eprintln!("vmd: stdin: {e}");
      return ExitCode::from(1);
    }
    (buf, String::from("stdin"), None, None, None, false)
  } else if let Some(arg) = path {
    // Split off `#anchor` from the path arg before touching the
    // filesystem. `vmd file.md#section` opens the file and scrolls
    // to the matching heading.
    let (path_str, anchor) = match arg.rsplit_once('#') {
      Some((p, a)) if !a.is_empty() => (p.to_string(), Some(a.to_string())),
      _ => (arg, None),
    };
    let p = match std::fs::canonicalize(&path_str) {
      Ok(p) => p,
      Err(e) => {
        eprintln!("vmd: {path_str}: {e}");
        return ExitCode::from(1);
      }
    };
    let body = match std::fs::read_to_string(&p) {
      Ok(s) => s,
      Err(e) => {
        eprintln!("vmd: {}: {e}", p.display());
        return ExitCode::from(1);
      }
    };
    let title = file_title(&p);
    let base_dir = p.parent().map(|d| d.to_path_buf());
    let ext_json = matches!(
      p.extension().and_then(|e| e.to_str()),
      Some("json" | "jsonc" | "json5"),
    );
    (body, title, watch.then_some(p), base_dir, anchor, ext_json)
  } else {
    usage();
  };

  // Mode resolution: --json forces, then extension, then stdin
  // content-sniff. Once decided, the flag travels with the viewer so
  // `--watch` reloads round-trip through the same path.
  let json_mode = force_json || ext_is_json || (from_stdin && vmd::json::looks_like_json(&source));

  crate::trace!("source_read");
  vmd::run(source, title, watch_path, base_dir, anchor, json_mode);
  ExitCode::SUCCESS
}

fn file_title(p: &Path) -> String {
  p.file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_else(|| String::from("vmd"))
}
