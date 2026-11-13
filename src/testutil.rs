//! Helpers shared by unit tests.
//!
//! The process environment is global while `cargo test` runs modules in
//! parallel, so every test that writes a variable — the image protocol, the
//! trash root, the terminal colour depth — plus every test that *reads* one
//! must share a single lock. Without it a test that restores `COLORTERM` can
//! run in the middle of a rendering test and make it fail intermittently.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialise tests that touch the process environment.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets environment variables for the duration of a test and puts them back.
///
/// Holding the env lock inside the guard means a test cannot forget to release
/// it, and the original values are restored even when an assertion panics.
pub struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Take the env lock and set `vars`.
    pub fn set(vars: &[(&str, &str)]) -> Self {
        let lock = env_lock();
        let mut saved = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            saved.push(((*key).to_string(), std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        Self { saved, _lock: lock }
    }

    /// Take the env lock and remove `keys` for the duration of the test.
    #[allow(dead_code)]
    pub fn unset(keys: &[&str]) -> Self {
        let lock = env_lock();
        let mut saved = Vec::with_capacity(keys.len());
        for key in keys {
            saved.push(((*key).to_string(), std::env::var_os(key)));
            std::env::remove_var(key);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}
