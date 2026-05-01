# md-view — second draft (research-backed)

Synthesizes findings from four parallel research streams (Tauri scaffolding, Rust markdown, CLI/Linux integration, frontend UX) as of 2026-05-01.

## Final stack

| Layer | Choice | Version | Why |
|---|---|---|---|
| App framework | Tauri 2 | `2.11` | Current stable. No 3.x in beta. v1 allowlist replaced by capabilities — config schema differs. |
| Frontend bundler | Vite | `5.4` (or `6`) | Tauri-recommended. No Tauri-specific Vite plugin needed. |
| Frontend | Vanilla TS | `typescript ^5.6` | No framework. Static viewer. |
| Markdown parser | `pulldown-cmark` | `0.13` | Event-stream API plugs cleanly into syntect. Lightest compile, GFM via `Options::ENABLE_GFM` etc. |
| Syntax highlighting | `syntect` + `two-face` | `5.2` + `0.4` | `ClassedHTMLGenerator` emits class names → light/dark swap is a CSS swap, no re-render. |
| GitHub-style CSS | `github-markdown-css` | `^5.8` | Drop-in `.markdown-body` class. ~16KB. Replaces "hand-rolled" plan. |
| External links | `@tauri-apps/plugin-opener` | `^2` | Renamed split from `plugin-shell`. The 2026 way to open URLs in system browser. |

## Why no frontend framework

Considered and rejected:

- **React** — ~45KB gzipped runtime, justified by ecosystem (TanStack, Radix, etc.) which we don't need. One-shot HTML inject + a few click handlers doesn't earn a VDOM.
- **Preact** — ~3KB, the right answer *if* a framework were needed. Same hooks/JSX as React. Revisit only if the viewer grows non-trivial state (multi-window nav, settings panel, file tree).
- **Yew (Rust → WASM)** — would *slow* startup, not speed it. Adds a 200KB+ WASM blob that downloads, compiles, and instantiates in the webview before paint. Webview cold-start (~150-300ms) already dominates; Yew adds to it. DOM ops still cross a JS bridge, and the markdown rendering is already in Rust on the backend — Yew would duplicate the language without removing the IPC boundary. Worth it for apps with heavy frontend compute (editors, CAD, spreadsheets); not for a static viewer.

Vite stays — it's the bundler/dev server, framework-agnostic. Earns its place via HMR, TS transpile, prod minify, and the official Tauri config (`port: 1420`, env prefix, etc.).

## Architecture changes from first draft

1. **Class-based code highlighting, not inline styles.** Use `syntect::html::ClassedHTMLGenerator` so dark/light is a stylesheet swap, not a re-render. Ship two `.tmTheme`-derived CSS files keyed off `html[data-theme]`.
2. **Skip hand-rolled CSS** — pull in `github-markdown-css`. Faster, faithful, auto-tracks GH primer tokens.
3. **CLI validation happens *before* `tauri::Builder`.** stderr + non-zero exit if file missing — no window flashes.
4. **Stdin support is ~3 lines** (`mdv -`). Worth including.
5. **Linux distribution: raw binary, not AppImage.** AppImage adds 100-300ms startup latency via FUSE — kills `mdv file.md` muscle memory. Symlink `target/release/md-view` directly into `~/.local/bin/mdv`.
6. **NVIDIA + Wayland gotcha.** Webkit2gtk-4.1 on NVIDIA proprietary drivers (the 5090 here) is flaky. Wrap launches with `WEBKIT_DISABLE_DMABUF_RENDERER=1` — common workaround. Bake into the install symlink as a wrapper script if blank-window issues appear.

## Project layout

```
md-view/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── capabilities/default.json
│   ├── icons/
│   └── src/
│       ├── main.rs        # CLI parse → exit-or-launch
│       ├── lib.rs         # tauri::Builder wiring, commands
│       └── render.rs      # pulldown-cmark + syntect pipeline
├── src/
│   ├── index.html
│   ├── main.ts            # invoke('load_file'), inject HTML, theme toggle, link handler
│   └── style.css          # @import github-markdown-css + syntect themes + dark/light swap
├── package.json
├── vite.config.ts
├── tsconfig.json
└── install.sh             # cargo tauri build + symlink + .desktop install
```

## Pinned dependencies

`src-tauri/Cargo.toml`
```toml
[build-dependencies]
tauri-build = { version = "2.6", features = [] }

[dependencies]
tauri          = { version = "2.11", features = [] }
tauri-plugin-opener = "2"
serde          = { version = "1", features = ["derive"] }
serde_json     = "1"
pulldown-cmark = { version = "0.13", default-features = false, features = ["html"] }
syntect        = { version = "5.2", default-features = false, features = ["default-fancy"] }
two-face       = "0.4"
```

`package.json`
```json
{
  "dependencies": {
    "@tauri-apps/api": "2.11.0",
    "@tauri-apps/plugin-opener": "^2",
    "github-markdown-css": "^5.8"
  },
  "devDependencies": {
    "@tauri-apps/cli": "2.11.0",
    "typescript": "5.6.3",
    "vite": "5.4.10"
  }
}
```

## CLI flow (Rust)

```rust
// src-tauri/src/main.rs
fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: mdv <file|->");
        std::process::exit(2);
    });
    let (src, title) = if arg == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).unwrap();
        (s, "stdin".to_string())
    } else {
        let p = std::fs::canonicalize(&arg).unwrap_or_else(|e| {
            eprintln!("mdv: {arg}: {e}"); std::process::exit(1);
        });
        let s = std::fs::read_to_string(&p).unwrap_or_else(|e| {
            eprintln!("mdv: {}: {e}", p.display()); std::process::exit(1);
        });
        let title = p.file_name().unwrap().to_string_lossy().into_owned();
        (s, title)
    };
    md_view_lib::run(src, title);
}
```

## Rendering pipeline (Rust)

```rust
// src-tauri/src/render.rs — sketch
use pulldown_cmark::{Parser, Options, Event, Tag, html};
use syntect::{parsing::SyntaxSet, html::{ClassedHTMLGenerator, ClassStyle}, util::LinesWithEndings};

pub fn render(md: &str, ss: &SyntaxSet) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
             | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_GFM;
    let parser = Parser::new_ext(md, opts);
    let events = highlight_code_blocks(parser, ss);  // intercept Tag::CodeBlock(Fenced(lang))
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}
// classed output → CSS owns light/dark
```

## tauri.conf.json (single window viewer)

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "productName": "md-view",
  "version": "0.1.0",
  "identifier": "dev.immel.mdv",
  "build": {
    "devUrl": "http://localhost:1420",
    "frontendDist": "../dist",
    "beforeDevCommand": "pnpm dev",
    "beforeBuildCommand": "pnpm build"
  },
  "app": {
    "windows": [{ "label": "main", "title": "mdv", "width": 900, "height": 1100, "resizable": true }],
    "security": {
      "csp": "default-src 'self'; img-src 'self' asset: data:; style-src 'self' 'unsafe-inline'; script-src 'self'",
      "assetProtocol": { "enable": true, "scope": [] }
    }
  },
  "bundle": { "active": true, "targets": ["deb"], "category": "Utility", "icon": ["icons/icon.png"] }
}
```
`assetProtocol.scope` populated dynamically per-doc (parent dir of opened .md) — narrow scope, not `**`.

## Frontend (TS) — what `main.ts` does

1. `invoke('load_file')` → `{ html, title }`. Inject into `<article class="markdown-body">`.
2. `document.title = title`.
3. Theme: read `localStorage.theme` else system pref → set `html[data-theme]`. Toggle on `Ctrl+T` or button.
4. Link click delegation: `https?://` → `opener.openUrl()` + preventDefault; `#anchor` → native; relative `.md` → invoke `open_in_new_window` (Rust spawns `WebviewWindowBuilder` with new file path).
5. Code blocks: language label from `class="language-xxx"`, copy button with `navigator.clipboard.writeText`. ~10 lines TS, no lib.

## Sanitization

Skip `ammonia`. Local files only, strict CSP, and pulldown-cmark's `Event::Html` pass-through is filtered out in the event stream (drop raw HTML) — covers the threat model.

## Linux file association

`~/.local/share/applications/mdv.desktop`:
```ini
[Desktop Entry]
Type=Application
Name=mdv
Exec=mdv %f
MimeType=text/markdown;
NoDisplay=true
Terminal=false
```

`install.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
pnpm install
pnpm tauri build --bundles none
ln -sf "$PWD/src-tauri/target/release/md-view" ~/.local/bin/mdv
mkdir -p ~/.local/share/applications
cp mdv.desktop ~/.local/share/applications/
update-desktop-database ~/.local/share/applications
xdg-mime default mdv.desktop text/markdown
```

## Deferred / explicitly out of scope

- **Math (KaTeX), mermaid** — heavy (280KB / 1.2MB). Defer behind dynamic `import()` triggered only when matching nodes exist. Zero cost when absent.
- **Live reload** (`notify` crate) — add later.
- **Single-instance / `--new-window`** — default is one-window-per-invocation (matches `mpv`, `xdg-open`). Revisit if needed.
- **Floating TOC, vim-style j/k** — browser scrolling and native Ctrl+F are fine.
- **Line numbers in code blocks** — visual noise for "basic". Skip.

## Sources

- [Tauri 2 release index](https://v2.tauri.app/release/)
- [Tauri 2 Vite guide](https://v2.tauri.app/start/frontend/vite/)
- [Tauri 2 capabilities](https://v2.tauri.app/security/capabilities/), [CSP](https://v2.tauri.app/security/csp/)
- [pulldown-cmark](https://crates.io/crates/pulldown-cmark) · [syntect](https://crates.io/crates/syntect) · [two-face](https://crates.io/crates/two-face)
- [github-markdown-css](https://github.com/sindresorhus/github-markdown-css)
- [tauri-plugin-opener](https://v2.tauri.app/plugin/opener/)
- [convertFileSrc / asset protocol](https://v2.tauri.app/reference/javascript/api/namespacecore/#convertfilesrc)
- [freedesktop entry spec](https://specifications.freedesktop.org/desktop-entry-spec/latest/) · [Arch wiki: XDG MIME](https://wiki.archlinux.org/title/XDG_MIME_Applications)
