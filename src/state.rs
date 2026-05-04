use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Prefs {
  pub theme: Option<bool>,
  pub zoom: Option<f32>,
  /// Last window size in logical px. Restored on launch so vmd opens at
  /// the size the user left it. Tiling compositors override on map, so
  /// this only takes effect on stacking/floating WMs and non-Linux.
  pub width: Option<f32>,
  pub height: Option<f32>,
}

// Defends against stale prefs after a monitor hot-swap or a saved size
// from a different display. Logical px, so DPI changes don't affect the
// bounds.
const MIN_DIM: f32 = 200.0;
const MAX_W: f32 = 8000.0;
const MAX_H: f32 = 6000.0;

impl Prefs {
  pub fn empty() -> Self {
    Self {
      theme: None,
      zoom: None,
      width: None,
      height: None,
    }
  }
}

fn state_path() -> Option<PathBuf> {
  let base = if let Ok(s) = std::env::var("XDG_STATE_HOME") {
    PathBuf::from(s)
  } else {
    let home = std::env::var("HOME").ok()?;
    PathBuf::from(home).join(".local/state")
  };
  Some(base.join("vmd/prefs"))
}

pub fn load() -> Prefs {
  let Some(path) = state_path() else {
    return Prefs::empty();
  };
  let Ok(contents) = fs::read_to_string(&path) else {
    return Prefs::empty();
  };
  parse(&contents)
}

pub fn save(prefs: &Prefs) {
  let Some(path) = state_path() else { return };
  if let Some(parent) = path.parent() {
    let _ = fs::create_dir_all(parent);
  }
  let mut s = String::new();
  if let Some(dark) = prefs.theme {
    s.push_str(&format!("theme={}\n", if dark { "dark" } else { "light" }));
  }
  if let Some(z) = prefs.zoom {
    s.push_str(&format!("zoom={:.3}\n", z));
  }
  if let Some(w) = prefs.width {
    s.push_str(&format!("width={:.0}\n", w));
  }
  if let Some(h) = prefs.height {
    s.push_str(&format!("height={:.0}\n", h));
  }
  let _ = fs::write(&path, s);
}

fn parse(contents: &str) -> Prefs {
  let mut p = Prefs::empty();
  for line in contents.lines() {
    let Some((k, v)) = line.split_once('=') else {
      continue;
    };
    let v = v.trim();
    match k.trim() {
      "theme" => match v {
        "dark" => p.theme = Some(true),
        "light" => p.theme = Some(false),
        _ => {}
      },
      "zoom" => {
        if let Ok(z) = v.parse::<f32>() {
          if z > 0.0 {
            p.zoom = Some(z);
          }
        }
      }
      "width" => {
        if let Ok(w) = v.parse::<f32>() {
          if w.is_finite() && (MIN_DIM..=MAX_W).contains(&w) {
            p.width = Some(w);
          }
        }
      }
      "height" => {
        if let Ok(h) = v.parse::<f32>() {
          if h.is_finite() && (MIN_DIM..=MAX_H).contains(&h) {
            p.height = Some(h);
          }
        }
      }
      _ => {}
    }
  }
  p
}
