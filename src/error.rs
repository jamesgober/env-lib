//! Error type for every fallible operation in the crate.
//!
//! Variable *values* never appear in an error: they are often secrets
//! (tokens, passwords, connection strings), and errors end up in logs.
//! Errors name the variable, the file, and the line instead.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Shorthand for `Result<T, env_lib::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// Longest key echoed back in an error, in characters.
const KEY_PREVIEW: usize = 64;

/// Everything that can go wrong in `env-lib`.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A variable name was rejected.
    ///
    /// Names must start with an ASCII letter or `_` and contain only ASCII
    /// letters, digits, and `_`, so they are valid in every shell and on
    /// every platform. Fix the name; retrying fails the same way.
    InvalidKey {
        /// The rejected name, truncated.
        key: String,
        /// Which rule it broke.
        reason: &'static str,
    },

    /// A value was rejected for the named variable.
    ///
    /// Values may not contain NUL (no operating system can store it in an
    /// environment variable) and may not exceed
    /// [`MAX_VALUE_BYTES`](crate::MAX_VALUE_BYTES). The value itself is never
    /// included.
    InvalidValue {
        /// The variable the value was for.
        key: String,
        /// Which rule it broke.
        reason: &'static str,
    },

    /// A prefix passed to [`Env::with_prefix`](crate::Env::with_prefix) was
    /// rejected. Prefixes follow the same rules as names.
    InvalidPrefix {
        /// The rejected prefix, truncated.
        prefix: String,
        /// Which rule it broke.
        reason: &'static str,
    },

    /// A `.env` file could not be parsed.
    ///
    /// Reported with the 1-based line number so a hand edit is easy to
    /// find. The file was not loaded; fix the line and load again.
    Syntax {
        /// The file, when the input came from one.
        path: Option<PathBuf>,
        /// 1-based line number of the problem.
        line: usize,
        /// What is wrong with the line.
        reason: &'static str,
    },

    /// A variable's value could not be parsed into the requested type by
    /// [`Env::parse`](crate::Env::parse).
    Parse {
        /// The variable whose value did not parse.
        key: String,
        /// The type it was parsed as.
        type_name: &'static str,
    },

    /// [`Env::save`](crate::Env::save) or [`Env::reload`](crate::Env::reload)
    /// was called on an `Env` that is not tied to a file. Use
    /// [`Env::save_to`](crate::Env::save_to), or create it with
    /// [`Env::open`](crate::Env::open).
    NoFile,

    /// A file is larger than [`MAX_FILE_BYTES`](crate::MAX_FILE_BYTES).
    TooLarge {
        /// The file.
        path: PathBuf,
    },

    /// Locating or writing the file failed in `path-lib` (for example, the
    /// application directories could not be resolved, or an atomic write
    /// failed).
    Path(path_lib::Error),

    /// The operating system returned an I/O error.
    Io {
        /// The step that failed, such as `"read"`.
        op: &'static str,
        /// The path it failed on.
        path: PathBuf,
        /// The error reported by the operating system.
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn invalid_key(key: &str, reason: &'static str) -> Self {
        Self::InvalidKey {
            key: preview(key),
            reason,
        }
    }

    pub(crate) fn invalid_value(key: &str, reason: &'static str) -> Self {
        Self::InvalidValue {
            key: preview(key),
            reason,
        }
    }

    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            op,
            path: path.into(),
            source,
        }
    }
}

impl From<path_lib::Error> for Error {
    fn from(err: path_lib::Error) -> Self {
        Self::Path(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey { key, reason } => {
                write!(f, "invalid variable name {key:?}: {reason}")
            }
            Self::InvalidValue { key, reason } => {
                write!(f, "invalid value for {key:?}: {reason}")
            }
            Self::InvalidPrefix { prefix, reason } => {
                write!(f, "invalid prefix {prefix:?}: {reason}")
            }
            Self::Syntax { path, line, reason } => match path {
                Some(path) => write!(f, "{}:{line}: {reason}", path.display()),
                None => write!(f, "line {line}: {reason}"),
            },
            Self::Parse { key, type_name } => {
                write!(f, "the value of {key:?} is not a valid {type_name}")
            }
            Self::NoFile => f.write_str("this Env is not tied to a file; use save_to"),
            Self::TooLarge { path } => write!(
                f,
                "{} is larger than the {} byte limit",
                path.display(),
                crate::MAX_FILE_BYTES
            ),
            Self::Path(err) => fmt::Display::fmt(err, f),
            Self::Io { op, path, source } => write!(f, "cannot {op} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Path(err) => Some(err),
            _ => None,
        }
    }
}

/// Truncates a name for inclusion in an error.
fn preview(s: &str) -> String {
    match s.char_indices().nth(KEY_PREVIEW) {
        Some((cut, _)) => format!("{}...", &s[..cut]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_display_names_subject_for_every_variant() {
        let cases: [(Error, &str); 9] = [
            (
                Error::invalid_key("1X", "r"),
                "invalid variable name \"1X\": r",
            ),
            (Error::invalid_value("K", "r"), "invalid value for \"K\": r"),
            (
                Error::InvalidPrefix {
                    prefix: "x-".into(),
                    reason: "r",
                },
                "invalid prefix \"x-\": r",
            ),
            (
                Error::Syntax {
                    path: Some(".env".into()),
                    line: 3,
                    reason: "r",
                },
                ".env:3: r",
            ),
            (
                Error::Syntax {
                    path: None,
                    line: 3,
                    reason: "r",
                },
                "line 3: r",
            ),
            (
                Error::Parse {
                    key: "PORT".into(),
                    type_name: "u16",
                },
                "not a valid u16",
            ),
            (Error::NoFile, "not tied to a file"),
            (Error::TooLarge { path: "big".into() }, "byte limit"),
            (
                Error::io("read", "f", io::Error::other("boom")),
                "cannot read f: boom",
            ),
        ];
        for (err, needle) in cases {
            assert!(err.to_string().contains(needle), "{err}");
        }
    }

    #[test]
    fn test_path_errors_convert_and_chain() {
        use std::error::Error as _;
        let err: Error = path_lib::Error::Unresolved { dir: "home" }.into();
        assert!(err.to_string().contains("home"));
        assert!(err.source().is_some());
        assert!(Error::NoFile.source().is_none());
    }

    #[test]
    fn test_long_keys_are_truncated_in_errors() {
        let long = "K".repeat(500);
        let shown = Error::invalid_key(&long, "r").to_string();
        assert!(shown.len() < 120, "{shown}");
        assert!(shown.contains("..."));
    }
}
