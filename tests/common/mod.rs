#[allow(dead_code)]
pub mod fixtures;
#[allow(dead_code)]
pub mod server;

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::marker::PhantomData;
use std::net::TcpListener;
use std::rc::Rc;
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

/// Global mutex that serialises tests which mutate process environment
/// variables. Tests that call [`set_env_var`] hold this lock for their
/// entire lifetime, preventing concurrent env-var mutations.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static ENV_LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
    static ENV_LOCK_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

fn acquire_env_lock() {
    ENV_LOCK_GUARD.with(|guard_cell| {
        if guard_cell.borrow().is_none() {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            *guard_cell.borrow_mut() = Some(guard);
        }
    });
    ENV_LOCK_DEPTH.with(|depth| depth.set(depth.get() + 1));
}

fn release_env_lock() {
    ENV_LOCK_DEPTH.with(|depth| {
        let current = depth.get();
        debug_assert!(current > 0, "env lock depth underflow");
        let next = current.saturating_sub(1);
        depth.set(next);
        if next == 0 {
            ENV_LOCK_GUARD.with(|guard_cell| {
                guard_cell.borrow_mut().take();
            });
        }
    });
}

/// RAII guard that mutates one environment variable on creation and restores
/// its previous state on drop. Also holds the global env lock so that only one
/// test at a time can mutate the process environment.
#[allow(dead_code)]
pub struct EnvVarGuard {
    name: &'static str,
    previous: Option<OsString>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // Safety: env mutation — serialised via ENV_LOCK.
        unsafe {
            match &self.previous {
                Some(previous) => std::env::set_var(self.name, previous),
                None => std::env::remove_var(self.name),
            }
        }
        release_env_lock();
    }
}

#[allow(dead_code)]
pub type RootEnvGuard = EnvVarGuard;

/// Set `name` to `value`, returning a guard that restores the previous value on
/// drop (even on panic) and serialises access across concurrent tests.
#[allow(dead_code)]
pub fn set_env_var(name: &'static str, value: Option<&str>) -> EnvVarGuard {
    acquire_env_lock();
    let previous = std::env::var_os(name);
    // Safety: env mutation — serialised via ENV_LOCK.
    unsafe {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    EnvVarGuard {
        name,
        previous,
        _not_send: PhantomData,
    }
}

/// Set the `ROOT` env var to `root`, returning a guard that removes it on
/// drop (even on panic) and serialises access across concurrent tests.
#[allow(dead_code)]
pub fn set_root_env(root: &std::path::Path) -> RootEnvGuard {
    set_env_var("ROOT", Some(root.to_str().unwrap()))
}
