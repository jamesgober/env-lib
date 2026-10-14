//! Rules for variable names, values, and the namespace prefix.
//!
//! Names follow the POSIX portable set: an ASCII letter or `_`, then
//! ASCII letters, digits, or `_`. Anything else (dots, dashes, spaces) is
//! either impossible to use from a shell or not portable to Windows, so it
//! is rejected rather than silently changed.

use crate::error::{Error, Result};

/// The prefix applied when exporting to the process environment, used
/// whenever no other prefix is configured (including an empty one).
pub const DEFAULT_PREFIX: &str = "JG_";

/// Longest accepted variable name, in bytes.
pub const MAX_KEY_BYTES: usize = 255;

/// Longest accepted value, in bytes. Windows caps an environment variable
/// at 32,767 UTF-16 units; staying within that in bytes keeps every value
/// exportable on every platform.
pub const MAX_VALUE_BYTES: usize = 32_767;

/// Validates a name against the portable rules. Returns the reason on
/// failure.
pub(crate) fn check_name(name: &str) -> std::result::Result<(), &'static str> {
    let bytes = name.as_bytes();
    let Some(first) = bytes.first() else {
        return Err("must not be empty");
    };
    if name.len() > MAX_KEY_BYTES {
        return Err("is longer than 255 bytes");
    }
    if !(first.is_ascii_alphabetic() || *first == b'_') {
        return Err("must start with an ASCII letter or `_`");
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return Err("may only contain ASCII letters, digits, and `_`");
    }
    Ok(())
}

/// Validates a key for storage.
pub(crate) fn check_key(key: &str) -> Result<()> {
    check_name(key).map_err(|reason| Error::invalid_key(key, reason))
}

/// Validates a value for the named key.
pub(crate) fn check_value(key: &str, value: &str) -> Result<()> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(Error::invalid_value(key, "is longer than 32,767 bytes"));
    }
    if value.contains('\0') {
        return Err(Error::invalid_value(key, "contains a NUL character"));
    }
    Ok(())
}

/// Normalizes a prefix: empty becomes [`DEFAULT_PREFIX`], anything else
/// must be a valid name.
pub(crate) fn normalize_prefix(prefix: &str) -> Result<String> {
    if prefix.is_empty() {
        return Ok(DEFAULT_PREFIX.to_owned());
    }
    check_name(prefix).map_err(|reason| Error::InvalidPrefix {
        prefix: prefix.chars().take(64).collect(),
        reason,
    })?;
    Ok(prefix.to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_check_name_accepts_portable_names() {
        for ok in [
            "A",
            "_",
            "PORT",
            "db_url",
            "X1",
            "_private",
            "a".repeat(255).as_str(),
        ] {
            assert_eq!(check_name(ok), Ok(()), "{ok:?}");
        }
    }

    #[test]
    fn test_check_name_rejects_everything_else() {
        let long = "a".repeat(256);
        for bad in [
            "",
            "1X",
            "A-B",
            "A.B",
            "A B",
            "É",
            "A=B",
            "A\0",
            long.as_str(),
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_check_value_limits() {
        assert!(check_value("K", "").is_ok());
        assert!(check_value("K", "multi\nline \"quoted\" = # ok").is_ok());
        assert!(check_value("K", &"v".repeat(MAX_VALUE_BYTES)).is_ok());
        assert!(check_value("K", &"v".repeat(MAX_VALUE_BYTES + 1)).is_err());
        assert!(check_value("K", "a\0b").is_err());
    }

    #[test]
    fn test_check_value_error_never_contains_value() {
        let err = check_value("TOKEN", "s3cr3t\0").unwrap_err().to_string();
        assert!(!err.contains("s3cr3t"), "{err}");
        assert!(err.contains("TOKEN"));
    }

    #[test]
    fn test_normalize_prefix_defaults_and_validates() {
        assert_eq!(normalize_prefix("").unwrap(), DEFAULT_PREFIX);
        assert_eq!(normalize_prefix("HIVE_").unwrap(), "HIVE_");
        assert_eq!(normalize_prefix("hive").unwrap(), "hive");
        for bad in ["JG.", "1_", "A-", " "] {
            assert!(
                matches!(normalize_prefix(bad), Err(Error::InvalidPrefix { .. })),
                "{bad:?}"
            );
        }
    }
}
