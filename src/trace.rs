use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub static APP_START: OnceLock<Instant> = OnceLock::new();
// AtomicBool rather than OnceLock<bool>: the flag must be flippable from
// both `init()` (env var, runs before argv parsing) and `enable()`
// (`--trace`, parsed after). A OnceLock locks in whichever wrote first
// and silently drops the other, so `vmd --trace` was a no-op when
// VMD_TRACE was unset.
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init() {
  let _ = APP_START.set(Instant::now());
  if std::env::var_os("VMD_TRACE").is_some() {
    ENABLED.store(true, Ordering::Relaxed);
  }
}

pub fn enable() {
  ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
  ENABLED.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::trace::enabled() {
            let elapsed = $crate::trace::APP_START
                .get()
                .map(|t| t.elapsed().as_micros() as f64 / 1000.0)
                .unwrap_or(0.0);
            eprintln!("[vmd] {:>7.3}ms {}", elapsed, format_args!($($arg)*));
        }
    };
}
