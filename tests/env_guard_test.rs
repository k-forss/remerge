mod common;

use std::sync::mpsc;
use std::time::Duration;

#[test]
fn set_env_var_allows_nested_guards() {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let _first = common::set_env_var("REMERGE_TEST_ENV_GUARD_ONE", Some("one"));
        let _second = common::set_env_var("REMERGE_TEST_ENV_GUARD_TWO", Some("two"));
        tx.send((
            std::env::var("REMERGE_TEST_ENV_GUARD_ONE").ok(),
            std::env::var("REMERGE_TEST_ENV_GUARD_TWO").ok(),
        ))
        .expect("send nested env state");
    });

    let values = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("nested env guards should not deadlock");

    assert_eq!(values.0.as_deref(), Some("one"));
    assert_eq!(values.1.as_deref(), Some("two"));
}
