use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ! {
    eprintln!("usage: mdv <file.md|->");
    std::process::exit(2);
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let arg = match args.next() {
        Some(a) => a,
        None => usage(),
    };

    let (source, title) = match arg.as_str() {
        "-" => {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("mdv: stdin: {e}");
                return ExitCode::from(1);
            }
            (buf, String::from("stdin"))
        }
        "-h" | "--help" => usage(),
        path => {
            let p = match std::fs::canonicalize(path) {
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
            let title = file_title(&p);
            (body, title)
        }
    };

    md_view_lib::run(source, title);
    ExitCode::SUCCESS
}

fn file_title(p: &PathBuf) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("mdv"))
}
