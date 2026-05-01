use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use mdv::trace;

fn usage() -> ! {
    eprintln!("usage: mdv [--licenses|--trace] <file.md|->");
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
