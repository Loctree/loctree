//! Per-test configuration injection, with no process-environment writes.
//!
//! Rust's test harness is multithreaded. Neither per-key locks nor a global
//! test mutex can make POSIX environment mutation safe against library readers.
//! Each test thread therefore owns an overlay used by the unit under test.
//! Production builds continue to read `std::env` directly. Child processes get
//! explicit `Command::env` / `env_remove` settings, never inherited mutations.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

thread_local! {
    static OVERRIDES: RefCell<BTreeMap<OsString, Option<OsString>>> = RefCell::default();
}

pub(crate) fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    OVERRIDES.with(|values| {
        values
            .borrow_mut()
            .insert(key.as_ref().into(), Some(value.as_ref().into()));
    });
}

pub(crate) fn remove_var<K: AsRef<OsStr>>(key: K) {
    OVERRIDES.with(|values| {
        values.borrow_mut().insert(key.as_ref().into(), None);
    });
}

#[cfg(feature = "aicx-inprocess")]
pub(crate) fn injected_value(key: &str) -> Option<OsString> {
    OVERRIDES.with(|values| values.borrow().get(OsStr::new(key)).cloned().flatten())
}

pub(crate) fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    OVERRIDES.with(|values| match values.borrow().get(key.as_ref()) {
        Some(value) => value.clone(),
        None => std::env::var_os(key),
    })
}

pub(crate) fn var<K: AsRef<OsStr>>(key: K) -> Result<String, std::env::VarError> {
    var_os(key)
        .ok_or(std::env::VarError::NotPresent)?
        .into_string()
        .map_err(std::env::VarError::NotUnicode)
}

pub(crate) fn apply(command: &mut std::process::Command) {
    OVERRIDES.with(|values| {
        for (key, value) in values.borrow().iter() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
    });
}

/// Capture injection when an operation deliberately moves to a worker thread.
#[cfg(feature = "aicx-inprocess")]
pub(crate) fn inherit<F: FnOnce() -> T, T>(operation: F) -> impl FnOnce() -> T {
    let captured = OVERRIDES.with(|values| values.borrow().clone());
    move || {
        OVERRIDES.with(|values| *values.borrow_mut() = captured);
        operation()
    }
}

#[test]
fn overrides_are_local_and_do_not_mutate_the_process() {
    const KEY: &str = "LOCT_TEST_ENV_ISOLATION";
    let before = std::env::var_os(KEY);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|value| {
            let barrier = barrier.clone();
            let before = before.clone();
            std::thread::spawn(move || {
                set_var(KEY, value);
                barrier.wait();
                assert_eq!(var(KEY).as_deref(), Ok(value));
                assert_eq!(std::env::var_os(KEY), before);
                remove_var(KEY);
                assert!(var(KEY).is_err());
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("test thread");
    }
    assert_eq!(std::env::var_os(KEY), before);
}

#[cfg(unix)]
#[test]
fn child_configuration_is_explicit_and_preserves_parent_environment() {
    const KEY: &str = "LOCT_TEST_CHILD_ENV";
    let before = std::env::var_os(KEY);
    set_var(KEY, "child-value");
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "printf %s \"$LOCT_TEST_CHILD_ENV\""]);
    apply(&mut command);
    let output = command.output().expect("child process");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"child-value");
    assert_eq!(std::env::var_os(KEY), before);
    remove_var(KEY);
    let mut command = std::process::Command::new("sh");
    command
        .env(KEY, "must-be-removed")
        .args(["-c", "test -z \"${LOCT_TEST_CHILD_ENV+x}\""]);
    apply(&mut command);
    assert!(command.status().expect("child process").success());
}

#[cfg(feature = "aicx-inprocess")]
#[test]
fn spawned_operation_keeps_its_captured_configuration() {
    const KEY: &str = "LOCT_TEST_CAPTURED_ENV";
    let before = std::env::var_os(KEY);
    set_var(KEY, "captured");
    let operation = inherit(|| var(KEY));
    set_var(KEY, "later-parent-setting");
    let observed = std::thread::spawn(operation).join().expect("worker");
    assert_eq!(observed.as_deref(), Ok("captured"));
    assert_eq!(var(KEY).as_deref(), Ok("later-parent-setting"));
    assert_eq!(std::env::var_os(KEY), before);
    remove_var(KEY);
}
