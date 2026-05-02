use std::sync::OnceLock;
use std::time::Instant;

pub static APP_START: OnceLock<Instant> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

pub fn init() {
    let _ = APP_START.set(Instant::now());
    let _ = ENABLED.set(std::env::var_os("VMD_TRACE").is_some());
}

pub fn enable() {
    let _ = ENABLED.set(true);
}

pub fn enabled() -> bool {
    *ENABLED.get().unwrap_or(&false)
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
