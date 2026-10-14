//! `Env::export` writes the process environment, which is only sound with
//! no other threads touching it. This binary holds exactly one test, so
//! the harness runs nothing else concurrently.

#![allow(clippy::unwrap_used, missing_docs, unsafe_code)]

use env_lib::Env;

#[test]
fn test_export_sets_and_clears_prefixed_variables_and_get_sees_them() {
    let mut env = Env::new().with_prefix("ENVLIBTEST_").unwrap();
    env.set("PORT", "7000").unwrap();
    env.set("GONE", "x").unwrap();
    // SAFETY: the only test in this binary; no other thread touches the
    // environment.
    unsafe { env.export() };
    assert_eq!(std::env::var("ENVLIBTEST_PORT").unwrap(), "7000");
    assert_eq!(std::env::var("ENVLIBTEST_GONE").unwrap(), "x");

    assert!(env.remove("GONE"));
    // SAFETY: as above.
    unsafe { env.export() };
    assert!(std::env::var("ENVLIBTEST_GONE").is_err());

    // A fresh Env without the value finds it through the prefix.
    let other = Env::new().with_prefix("ENVLIBTEST_").unwrap();
    assert_eq!(other.get("PORT").as_deref(), Some("7000"));
    assert_eq!(other.get("ENVLIBTEST_PORT").as_deref(), Some("7000"));
    assert_eq!(other.parse::<u16>("PORT").unwrap(), Some(7000));
    // Never copied in: an Env holds only its own variables.
    assert!(other.is_empty());
}
