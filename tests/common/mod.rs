#[allow(dead_code)]
pub mod fixtures;
#[allow(dead_code)]
pub mod server;

use std::net::TcpListener;
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
