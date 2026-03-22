#[allow(dead_code)]
pub mod fixtures;
#[allow(dead_code)]
pub mod server;

use std::net::TcpListener;
use std::sync::MutexGuard;
use std::time::Duration;

/// Allocate a random free TCP port and return it.
#[allow(dead_code)]
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind to random port");
    listener.local_addr().unwrap().port()
}

/// Assert with a timeout — panics if the future doesn't resolve.
#[allow(dead_code)]
pub async fn with_timeout<F: std::future::Future>(duration: Duration, f: F) -> F::Output {
    tokio::time::timeout(duration, f)
        .await
        .expect("operation timed out")
}

/// Global mutex that serialises tests which mutate the `ROOT` environment
/// variable. Tests that call [`set_root_env`] hold this lock for their
/// entire lifetime, preventing concurrent env-var mutations.
static ROOT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that sets `ROOT` on creation and removes it on drop.
/// Also holds the [`ROOT_ENV_LOCK`] so that only one test at a time can
/// mutate the environment.
#[allow(dead_code)]
pub struct RootEnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl Drop for RootEnvGuard {
    fn drop(&mut self) {
        // Safety: env mutation — serialised via ROOT_ENV_LOCK.
        unsafe {
            std::env::remove_var("ROOT");
        }
    }
}

/// Set the `ROOT` env var to `root`, returning a guard that removes it on
/// drop (even on panic) and serialises access across concurrent tests.
#[allow(dead_code)]
pub fn set_root_env(root: &std::path::Path) -> RootEnvGuard {
    let lock = ROOT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Safety: env mutation — serialised via ROOT_ENV_LOCK.
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
    RootEnvGuard { _lock: lock }
}
