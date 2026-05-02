use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Prefs {
  pub theme: Option<bool>,
  pub zoom: Option<f32>,
}

impl Prefs {
  pub fn empty() -> Self {
    Self {
      theme: None,
      zoom: None,
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
      _ => {}
    }
  }
  p
}
