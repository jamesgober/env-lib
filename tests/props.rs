//! Property tests for the file format.

#![allow(clippy::unwrap_used, missing_docs)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use env_lib::Env;
use proptest::prelude::*;

fn name() -> impl Strategy<Value = String> {
    "[A-Za-z_][A-Za-z0-9_]{0,20}"
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ -~]{0,40}",
        any::<String>().prop_map(|s| s.replace('\0', "")),
        prop::collection::vec(
            prop_oneof![
                Just("\n"),
                Just("\r"),
                Just("\t"),
                Just("\""),
                Just("'"),
                Just("\\"),
                Just("$"),
                Just("#"),
                Just(" "),
                Just("="),
                Just("x"),
            ],
            0..12
        )
        .prop_map(|v| v.concat()),
    ]
}

fn scratch_file() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir()
        .join(format!("env-lib-prop-{}", std::process::id()))
        .join(format!("{}.env", N.fetch_add(1, Ordering::Relaxed)))
}

fn env_of(vars: &[(String, String)]) -> Env {
    let mut env = Env::new();
    for (k, v) in vars {
        env.set(k, v.clone()).unwrap();
    }
    env
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    /// Any set of variables survives saving and loading unchanged.
    #[test]
    fn prop_save_load_round_trip(vars in prop::collection::vec((name(), value()), 0..16)) {
        let mut env = env_of(&vars);
        let file = scratch_file();
        env.save_to(&file).unwrap();
        let back = Env::open(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        prop_assert_eq!(back.iter().collect::<Vec<_>>(), env.iter().collect::<Vec<_>>());
    }

    /// The parser never panics on arbitrary text.
    #[test]
    fn prop_parse_never_panics(text in any::<String>()) {
        let _ = Env::from_text(&text);
    }

    /// Opening and saving without changes leaves the file byte for byte
    /// the same.
    #[test]
    fn prop_unchanged_save_is_identity(vars in prop::collection::vec((name(), value()), 0..8)) {
        let mut env = env_of(&vars);
        let file = scratch_file();
        env.save_to(&file).unwrap();
        let first = std::fs::read(&file).unwrap();
        let mut reopened = Env::open(&file).unwrap();
        reopened.save().unwrap();
        let second = std::fs::read(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        prop_assert_eq!(first, second);
    }
}
