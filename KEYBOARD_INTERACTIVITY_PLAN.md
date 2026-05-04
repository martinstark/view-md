# Keyboard Interactivity (vimium-style hint mode)

Replaces the current `f` → "page down" shortcut. Pressing `f` overlays a
unique label on every interactive element that meets the visibility
filter; typing a label fires that element's action and closes the
overlay. `Esc` aborts.

Three categories of interactive element are hinted with a single,
unified badge style:

| Category    | Source in `LaidDoc`                                    | Action when fired                       |
| ----------- | ------------------------------------------------------ | --------------------------------------- |
| Links       | `LaidKind::Text.links` and `TableCellLayout.links`     | `App::follow_link(target)` (existing)   |
| Footnotes   | Same `LinkRange`s with `LinkTarget::Footnote` / `FootnoteBack` (no separate handling needed — they already flow through `follow_link`) | `App::follow_link(target)` (existing) |
| Code blocks | `LaidKind::CodeBlock { source, .. }`                   | New: copy `source` to clipboard         |
| Inline code | `LaidKind::Text.code_runs` and `TableCellLayout.code_runs` (substring extracted from buffer line text) | New: copy substring to clipboard |

The footnote back-arrow rows in `layout_footnotes` are already a single
`LinkRange::FootnoteBack` covering the whole label-row buffer, so they
yield exactly one hint per row with no special-casing.

## Visibility filter

Compute once when `f` is pressed. Targets are anchored in **document
coordinates** at capture time; the overlay is then frozen against
scroll/resize (see "Lifecycle" below), so positions never go stale
while the overlay is open.

Inputs: `laid: &LaidDoc`, `scroll_y`, `viewport_h`.

For each `LaidBlock` whose viewport-projected rect is even partially
visible (`block.y + block.h > scroll_y && block.y < scroll_y +
viewport_h`), inspect its `LaidKind`:

- **`CodeBlock`** — eligible iff
  `intersection_height(block, viewport) / block.h >= 0.70`.
- **`Text`** — for each `LinkRange` and each `UnderlineRun` in
  `code_runs`, walk `buffer.layout_runs()` and collect the union of
  per-glyph rects whose `(start, end)` overlap the byte range
  (`(g.x, g.x + g.w, run.line_top, line_top + line_height)`).
  Eligible iff **at least one** of those per-line rects is fully inside
  the viewport (top ≥ 0 and bottom ≤ viewport_h after subtracting
  `scroll_y` and adding `block.y`).
- **`Table`** — for each `TableRowLayout` whose row sits within the
  block's vertical range, then each `cell.links` and `cell.code_runs`,
  apply the same per-line-rect rule. Cell coordinates translate by
  `block.x + cell.x` and `block.y + row.y_top + pad_y`.
- All other `LaidKind`s contribute nothing.

The "first fully-visible per-line rect" is the badge anchor for links /
inline code (see next section).

If the resulting target list is empty: **do nothing** — `f` is a no-op
in this state. The overlay never opens with zero targets.

## Badge placement

For each eligible target, the badge top-left is computed once and
stored in document-space coordinates (i.e. relative to `scroll_y = 0`),
so paint time can render it at `(badge_x, badge_y - scroll_y)` even
though we never actually re-scroll while the overlay is open.

**Common rule:** `badge.y` is clamped so the badge is fully inside the
viewport with a small inner margin (`HINT_MARGIN = 4 px * scale`), even
if the natural anchor would draw it partially off-screen.

| Target kind        | Natural anchor                                         |
| ------------------ | ------------------------------------------------------ |
| Code block         | `(block.x, block.y)` — top-left of the block           |
| Link / inline code | `(rect.x, rect.y)` of the **first fully-visible** per-line rect for that target |

After computing `(natural_x, natural_y)`:

```
let vp_top    = scroll_y + HINT_MARGIN
let vp_bottom = scroll_y + viewport_h - HINT_MARGIN - badge_h
badge_y = natural_y.clamp(vp_top, vp_bottom)
badge_x = natural_x.clamp(HINT_MARGIN, frame_w - HINT_MARGIN - badge_w)
```

The horizontal clamp is a safety net for very wide labels next to the
right edge; in practice it almost never kicks in.

## Label generation (vimium algorithm)

**Priority alphabet** (single source of truth; tweak in one place):

```
const HINT_ALPHABET: &str = "fjdkslaghrueiwoncmpvbtzxqy";
```

Home-row first (`fjdkls`), then near-row, then awkward keys last.
26 chars; case-insensitive matching at input time.

Given `n` targets and `K = HINT_ALPHABET.len()`:

```
if n <= K:
    labels = first n single chars from alphabet
else:
    // choose `s` single-letter labels and `L = K - s` leader letters
    // such that s + L*K >= n, minimizing the number of chord labels
    // first (i.e. maximize s).
    s = max(0, K * K - n) / (K - 1)        // closed form
    L = K - s
    single = alphabet[0..s].chars().map(|c| c.to_string())
    chord_seeds = alphabet[s..K]            // worst-ergonomic letters
    chords = for leader in chord_seeds:
                 for suffix in alphabet[0..K]:
                     yield format!("{leader}{suffix}")
    labels = single ++ chords.take(n - s)
```

This produces the vimium pattern: as many single letters as fit, then
2-char chords whose leaders are pulled from the *end* of the priority
alphabet (so good keys stay single-letter). Targets are assigned labels
in document order (top-to-bottom, left-to-right), so home-row keys land
on the elements the user is most likely scanning first.

No label is a prefix of another — single letters and chords share no
overlapping prefix because chord leaders are exactly the letters *not*
used as single-letter labels.

## State

```rust
// in app.rs
pub struct HintTarget {
    pub action: HintAction,
    pub badge_x: f32,    // doc-space (scroll_y = 0)
    pub badge_y: f32,
}

pub enum HintAction {
    FollowLink(LinkTarget),
    CopyCode(String),
}

pub struct HintState {
    pub targets: Vec<HintTarget>,
    pub labels: Vec<String>,    // same length as targets
    pub typed: String,          // uppercase prefix typed so far
}

// in App:
pub hint: Option<HintState>,
```

`CopyCode` carries the code as an owned `String` (cloned from
`source` for code blocks, or extracted via `BufferLine::text()`
sliced by `byte_start..byte_end` for inline code). Owning the text
detaches the action from `laid`, so a `--watch` reload during the
overlay's lifetime can't dangle.

Default-initialize `hint: None` in `lib.rs::run`.

## Lifecycle (Input FSM)

### Opening

`handle_key` catches `Key::Character("f")` (case-insensitive — note the
`F` shifted variant doesn't currently fire because `Character` is
lowercase by convention; check `key.as_ref()`).

When `hint.is_none()` AND `search.is_none()` AND `!help_visible`:

1. Build `targets` via the visibility filter.
2. If empty → return without opening (no-op).
3. Build `labels` via the algorithm above.
4. `self.hint = Some(HintState { targets, labels, typed: String::new() })`.
5. `request_redraw()`.

### While open: keyboard

Routed at the top of `handle_key`, **before** any other branch (mirrors
the existing `if self.search.is_some()` guard at `app.rs:516`):

| Key                                                | Effect                                                                  |
| -------------------------------------------------- | ----------------------------------------------------------------------- |
| `Esc`                                              | `hint = None; request_redraw()`                                         |
| `Backspace`                                        | If `typed.pop().is_some()` → `request_redraw()`                         |
| `Character(c)` where `c.to_ascii_lowercase()` is in `HINT_ALPHABET` | Append uppercase to `typed`; resolve (see below)         |
| Any other key (Enter, arrows, modifiers-only, etc.) | Swallow; do not close                                                  |

Modifier-bearing keystrokes (Ctrl/Alt + something) should be ignored —
otherwise something like `Ctrl+C` could appear as a `Character("c")`.
Keep the same guard `handle_search_key` uses
(`if self.modifiers.state().control_key() || .alt_key() { return }`).

#### Resolution after each character append

```
let typed_lc = state.typed.to_lowercase();
let exact_idx = state.labels.iter()
    .position(|l| l.eq_ignore_ascii_case(&state.typed));
if let Some(i) = exact_idx {
    fire(state.targets[i].action);   // see "Firing" below
    self.hint = None;
    return;
}
let any_prefix = state.labels.iter()
    .any(|l| l.to_lowercase().starts_with(&typed_lc));
if !any_prefix {
    self.hint = None;                // invalid keypress aborts
}
self.request_redraw();
```

The redraw filters labels that don't match `typed` (skips painting
their badges); matching labels are drawn with the typed prefix in a
muted color and the remainder in the badge's normal text color. Letting
the user *see* what they've typed is the whole point of narrow-as-you-
type.

#### Firing

```rust
match action {
    HintAction::FollowLink(t)   => self.follow_link(t),
    HintAction::CopyCode(code)  => self.set_clipboard(code),
}
```

Both methods already exist. `follow_link` handles `Url`, `Footnote`,
`FootnoteBack` — the same dispatch that mouse clicks use, so behavior
is consistent.

### While open: other events that close the overlay

| Event                              | Reason                                                                                              | Handling                          |
| ---------------------------------- | --------------------------------------------------------------------------------------------------- | --------------------------------- |
| `MouseInput::Pressed`              | A click is a clearer disambiguator than a hint label                                                | `hint = None` then fall through to existing click handling |
| `WindowEvent::Resized`             | Positions are stale after relayout                                                                  | `hint = None` first, then existing resize logic                  |
| `WindowEvent::ScaleFactorChanged`  | Same                                                                                                | Same                              |
| `AppEvent::Reload` (`--watch`)     | Document content + layout changed                                                                   | Inside `reload_from_disk`, set `hint = None` before relayout                  |
| `WindowEvent::MouseWheel`          | Frozen — swallow (don't scroll, don't close)                                                        | Early return at top of `MouseWheel` arm if `hint.is_some()` |
| `CursorMoved`                      | Don't update drag-selection state (which would visually conflict)                                   | Early return if `hint.is_some()` |
| `AppEvent::ImageReady`             | Layout unchanged; only triggers a redraw                                                            | No special handling — overlay re-paints correctly |
| Animation frame deadline           | Same                                                                                                | No special handling               |

`?` (help) and `/` (search) are normal `Character` keys; while the
hint overlay is open they're swallowed by the alphabet/Esc/Backspace
filter (Esc still closes hint, then a second keypress can open
help/search). This matches the user's spec: *"other key interactions
are not active while the overlay is open."*

### Help overlay text

Update `paint_help_overlay`'s `entries` list at `paint.rs:495`:

```rust
("f",            "follow link / copy code by hint"),
```

Replace the existing `("f / b / Space", "full page down / up")` line by
removing `f` from it (becomes `("b / Space", "full page down")` plus
add `("u / b", "...")` rebalancing). Also update the `paint_help_overlay`
column widths if needed to fit the new label.

## Painting

New `Painter::paint_hints(frame, theme, hint, scroll_y, scale)` called
from `App::redraw()` after the doc paint and any selection/search
overlays, but before the help overlay. Help overlay should still paint
on top (it's a modal that can't co-occur — actually it *can* co-occur
in theory if the user hits `?` from inside hint mode, which we already
swallow, so help-on-top just keeps the obvious layering invariant).

Style:

```rust
const BADGE_BG:     SkColor = SkColor::from_rgba8(0xff, 0xc8, 0x2a, 0xff); // saturated yellow
const BADGE_BORDER: SkColor = SkColor::from_rgba8(0x66, 0x49, 0x00, 0xff); // dark amber
const BADGE_FG:     Color   = Color::rgb(0x14, 0x14, 0x14);                // near-black
const BADGE_FG_DIM: Color   = Color::rgb(0x6b, 0x55, 0x10);                // typed-prefix color

const BADGE_FS:        f32 = 11.0;   // * scale
const BADGE_LH:        f32 = 14.0;   // * scale
const BADGE_PAD_X:     f32 =  4.0;   // * scale
const BADGE_PAD_Y:     f32 =  1.0;   // * scale
const BADGE_RADIUS:    f32 =  3.0;   // * scale
const BADGE_BORDER_W:  f32 =  1.0;   // * scale
const HINT_MARGIN:     f32 =  4.0;   // * scale
```

For each `(target, label)` where `label.starts_with(&hint.typed)` (case
insensitive):

1. Build a plain-text buffer for the label. Width = `label.len() *
   font_size` budget (loose; we measure after via `layout_runs`).
2. Measure the badge: `badge_w = measured_w + 2*BADGE_PAD_X`,
   `badge_h = BADGE_LH + 2*BADGE_PAD_Y`.
3. `(bx, by)` = clamped position from "Badge placement" applied to
   `(target.badge_x, target.badge_y - scroll_y)`.
4. `fill_rounded_rect_aa(frame, bx, by, badge_w, badge_h, BADGE_RADIUS, BADGE_BG)`.
5. `stroke_rounded_rect_aa(frame, bx, by, badge_w, badge_h, BADGE_RADIUS, BADGE_BORDER, BADGE_BORDER_W * scale)`.
6. Draw the label text:
   - If `hint.typed` is empty: one `draw_buffer` call with `BADGE_FG`.
   - Otherwise: build the buffer as a 2-run rich-text using
     `Buffer::set_rich_text` — the typed prefix in `BADGE_FG_DIM`,
     the remainder in `BADGE_FG`. Same approach as
     `build_footnote_label` at `layout.rs:705`.

No special z-ordering work needed: badges draw last (within doc
coordinates) and are small enough that pile-ups are rare. If two
badges' rects collide, accept the visual overlap for v1 — the hint
algorithm caps targets to a tractable count and overlap mostly happens
only when the doc is dense.

## File-by-file changes

### `src/app.rs`
1. Add `HintTarget`, `HintAction`, `HintState` types (top-level).
2. Add `pub hint: Option<HintState>` field to `App`.
3. New `App::open_hints()` — builds targets/labels, sets `self.hint`.
4. New `App::collect_hint_targets()` — visibility filter pass.
5. New `App::resolve_hint_input(...)` — the FSM resolver.
6. New `App::fire_hint_action(action)` — dispatches `FollowLink` /
   `CopyCode`.
7. In `handle_key`: add `if self.hint.is_some() { ... return }` guard
   (mirroring the search guard).
8. In `handle_key`: change the `Key::Character("f")` arm from
   "page down" to `self.open_hints()`.
9. In `window_event`'s `MouseWheel` / `MouseInput` /
   `CursorMoved` / `Resized` / `ScaleFactorChanged` arms: close hint
   first (or short-circuit for `MouseWheel`/`CursorMoved`).
10. In `user_event` / `reload_from_disk`: clear `self.hint` before
    relayout.
11. In `lib.rs::run`: initialize `hint: None`.
12. In `redraw()`: invoke `painter.paint_hints(...)` after the doc and
    selection paint, before help.

### `src/paint.rs`
1. New `Painter::paint_hints(...)` function (drawing logic above).
2. Constants block at the top of the file (or near other UI constants).
3. The function reuses `fill_rounded_rect_aa`,
   `stroke_rounded_rect_aa`, `make_plain_buffer`, and `draw_buffer`.
   No new tiny-skia paths required.

### `src/layout.rs`
No structural changes. Visibility-filter helpers (e.g.
`link_visual_rects(buffer, range, line_height) -> Vec<Rect>`) live in
`app.rs` since they're consumed there. If they're useful for the
painter too, lift them later.

### Help overlay (`paint.rs:495`)
Update the entries list as noted under "Help overlay text".

### Hint label generator
Inline helper in `app.rs`, ~30 lines. Pure function, easy to unit-test:

```rust
pub(crate) fn build_hint_labels(n: usize, alphabet: &str) -> Vec<String>
```

## Edge cases & explicit behaviors

- **`f` while search overlay is open:** swallowed by search's input
  capture (search treats it as a query character). No change.
- **`f` while help overlay is open:** swallowed by help's input filter.
  No change — close help first, then `f`.
- **Empty target list:** `open_hints()` returns early, leaves
  `self.hint = None`. No badge flash, no overlay state change.
- **A link wraps onto a partially-visible second line:** the rule says
  "at least one fully-visible per-line rect" → eligible; badge anchors
  to the first fully-visible line's start, not the partially visible
  one. This avoids badges that themselves get clipped.
- **Inline code spans without a `LinkRange`:** also hinted (action =
  copy substring). Confirmed scope.
- **Code block at exactly 70.0% visible:** boundary is `>=`, so it
  shows.
- **Code block whose top is in viewport but bottom is far below:**
  badge anchors at the natural top-left of the block (already inside
  viewport).
- **Resize-triggered close vs. resize-anchor scroll:** the resize path
  in `app.rs:396` will run after `hint = None`, and the existing
  capture/restore-anchor logic continues to work.
- **Repeated `f` press while overlay is open:** treated as a normal
  keystroke; if `f` is a hint label, it fires; if it's a chord-prefix,
  it narrows; otherwise it aborts. The user explicitly asked for this.
- **Mouse motion during hint mode:** `CursorMoved` returns early so
  drag-selection state can't be modified mid-overlay. Click-press still
  closes the overlay (and resumes normal click behavior next frame).
- **Animated images / `--watch` text-only changes:** only `Reload`
  closes the overlay; `ImageReady` and animation deadlines do not.
  Layout doesn't change in those cases, so positions stay valid.
- **Overlay survives a synthetic `request_redraw()`:** yes — by design,
  redraw just re-paints.

## Test plan

- **Manual, single-letter pool**: README.md (modest link count) →
  every link/code-block hit by single letter.
- **Manual, chord pool**: a doc with >26 interactive elements → first N
  on home-row, rest as chords starting from `z/x/q/y` etc.
- **Manual, 70% rule**: scroll a code block until just below 70%
  visible → its hint disappears; scroll to 70%+ → reappears.
- **Manual, link-wrap**: a link whose first line is clipped by the
  viewport top, second line fully visible → badge appears at the start
  of line 2.
- **Manual, footnotes**: trigger both a body `[^x]` ref hint and a
  back-arrow `↩` hint; verify they scroll to def and back to first
  ref respectively.
- **Manual, inline code copy**: hint an inline `` `code` `` span; paste
  into a terminal; verify the substring matches.
- **Manual, table cell links**: an inline link inside a table cell;
  hint should appear and `follow_link` should open it.
- **Manual, abort paths**: `Esc`, click anywhere, scroll-wheel
  (verify no scroll), resize the window — overlay closes in all four.
- **Unit, label generator**: test `build_hint_labels(n, "fjdksla")` for
  `n = 0,1,7,8,15,49,50` — assert prefix-freeness and counts.
