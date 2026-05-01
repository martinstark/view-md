# md-view — first draft plan

## Stack
- **Tauri 2** (Rust backend, webview frontend) — matches Rust preference, single binary, ~10MB
- **Frontend**: vanilla TS + Vite, no framework. It's a static viewer.
- **Markdown → HTML**: `pulldown-cmark` in Rust (fast, no JS deps, GFM support)
- **Syntax highlighting**: `syntect` in Rust at parse time (bundled themes), or `highlight.js` in frontend if you want lazier loading. Default to `syntect` — keeps everything server-side and consistent.
- **Styling**: hand-rolled CSS, GitHub-flavored look. Light/dark via `prefers-color-scheme`.

## CLI flow
1. `mdv path/to/file.md` invoked
2. Binary checks args via `std::env::args` (skip `tauri-plugin-cli` — overkill for one positional arg)
3. Resolve to absolute path, validate file exists & is readable, error to stderr otherwise
4. Pass path to Tauri builder via `.manage(AppState { file: PathBuf })`
5. Frontend on mount calls `invoke("load_file")` → Rust reads file, renders markdown, returns HTML + title
6. Window title set to filename

## Single-instance behavior
Skip `tauri-plugin-single-instance`. Each `mdv` invocation = new window. Simpler, matches how `mpv`/`xdg-open` behave. Revisit if you actually want focus-existing.

## Project layout
```
md-view/
├── Cargo.toml            # workspace? probably not needed
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs       # CLI parse + tauri::Builder
│       └── render.rs     # pulldown-cmark + syntect
├── src/                  # frontend
│   ├── index.html
│   ├── main.ts           # invoke("load_file"), inject HTML
│   └── style.css         # GH-style markdown CSS
├── package.json
├── vite.config.ts
└── install.sh            # cargo build --release && symlink to ~/dev/scripts/desktop
```

## Build & install
- `cargo tauri build` produces binary at `src-tauri/target/release/md-view`
- `install.sh`: build, symlink as `mdv` into a PATH dir (`~/dev/scripts/` already wired up — `~/dev/scripts/desktop/` looks like it's for desktop entries; the binary itself probably belongs in `~/.local/bin` or wherever PATH points). Confirm where the symlink goes before writing the script.
- Optional `.desktop` file so it shows up in launchers / file associations for `.md`

## Open questions
1. **Live reload on file change?** "Very basic" — default no. Add `notify` crate later if wanted.
2. **What features beyond CommonMark?** GFM tables/strikethrough/task-lists yes, math (KaTeX) and mermaid would each pull in JS — skip unless asked.
3. **Window behavior**: fixed size, remember last size, or fit-to-content? Default: 900x1100, resizable, no persistence.
4. **Where should the `mdv` binary live?** `~/.local/bin`, `~/dev/scripts/`, or somewhere else on PATH?
