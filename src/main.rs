use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use mdv::trace;

fn usage() -> ! {
    eprintln!(
        "mdv — minimal native markdown viewer\n\
         \n\
         usage: mdv [flags] <file.md | ->\n\
         \n\
         flags:\n\
         \x20\x20--licenses     print bundled font licenses (SIL OFL 1.1)\n\
         \x20\x20--trace        print timing breakdown (also: MDV_TRACE=1)\n\
         \x20\x20-h, --help     this message\n\
         \n\
         keybinds (press ? in the app for the full list):\n\
         \x20\x20q / Esc        quit\n\
         \x20\x20t              toggle theme\n\
         \x20\x20j k d u f b    scroll line / half / full page\n\
         \x20\x20g G            top / bottom\n\
         \x20\x20] [ }} {{      next/prev heading / block\n\
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
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--licenses" => {
                mdv::licenses::print();
                return ExitCode::SUCCESS;
            }
            "--trace" => {
                trace::enable();
            }
            "-h" | "--help" => usage(),
            "-" => from_stdin = true,
            s if s.starts_with("--") => {
                eprintln!("mdv: unknown flag: {s}");
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

    let (source, title) = if from_stdin {
        let mut buf = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
            eprintln!("mdv: stdin: {e}");
            return ExitCode::from(1);
        }
        (buf, String::from("stdin"))
    } else if let Some(path) = path {
        let p = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("mdv: {path}: {e}");
                return ExitCode::from(1);
            }
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mdv: {}: {e}", p.display());
                return ExitCode::from(1);
            }
        };
        (body, file_title(&p))
    } else {
        usage();
    };

    crate::trace!("source_read");
    mdv::run(source, title);
    ExitCode::SUCCESS
}

fn file_title(p: &PathBuf) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("mdv"))
}
