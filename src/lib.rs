//! # env-lib
//!
//! APPLICATION ENVIRONMENT VARIABLES FOR RUST
//!
//! Set, read, and remove your application's environment variables in
//! code, keep them in a `.env` file that survives restarts and hand edits,
//! and hand them to child processes or the process environment under a
//! namespace prefix. The system environment is read, never copied into
//! the file.
//!
//! ```
//! use env_lib::Env;
//!
//! # let dir = std::env::temp_dir().join("env-lib-doc-root");
//! # let file = dir.join(".env");
//! let mut env = Env::open(&file)?;        // loads it if it exists
//! env.set("PORT", "7000")?;
//! env.remove("OLD_FLAG");
//! env.save()?;                            // only what changed is rewritten
//!
//! // Next run, or after editing the file by hand:
//! let env = Env::open(&file)?;
//! let port: u16 = env.parse("PORT")?.unwrap_or(8080);
//! # assert_eq!(port, 7000);
//! # std::fs::remove_dir_all(&dir).ok();
//! # Ok::<(), env_lib::Error>(())
//! ```
//!
//! # Where the file lives
//!
//! [`Env::open`] takes any path (a leading `~` is expanded).
//! [`Env::for_app`] uses the application's config directory from
//! [`path_lib`], for example `~/.config/<app>/.env` on Linux or
//! `%APPDATA%\<app>\.env` on Windows.
//!
//! # Names and the prefix
//!
//! Names are plain in code and in the file (`PORT`). Only the copies placed
//! in a process environment carry the prefix (`JG_PORT` by default; change
//! it with [`Env::with_prefix`], and an empty prefix falls back to `JG_`).
//! [`Env::get`] looks in the `Env` first and then in the process
//! environment under the prefixed name, so a value exported earlier or set
//! by a parent process is found too.
//!
//! # Getting variables to other code
//!
//! - [`Env::get`], [`Env::get_or`], [`Env::parse`] read from the `Env`
//!   (safe, thread-safe through `&Env`).
//! - [`Env::apply_to`] adds the variables to a child process's
//!   [`Command`](std::process::Command) (safe).
//! - [`Env::export`] writes them into the current process environment for
//!   code that reads `std::env` directly. It is `unsafe` because changing
//!   the environment while another thread reads it is undefined behavior;
//!   call it at startup, before spawning threads.
//!
//! # The file format
//!
//! `NAME=value` lines with `#` comments, optional `export `, and single or
//! double quotes (double quotes support `\n`, `\r`, `\t`, `\"`, `\\`, `\$` and
//! may span lines). Saving keeps comments, blank lines, ordering, line
//! endings, and the exact text of untouched entries. Variable
//! interpolation (`${OTHER}`) is not performed, by design. Malformed lines
//! are reported with their line number rather than skipped.
//!
//! # Secrets
//!
//! Values never appear in `Debug` output or in errors. New files are
//! created owner-only (`0600`) on Unix, and saves are atomic.

#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::unreachable)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::missing_safety_doc)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

mod dotenv;
mod env;
mod error;
mod key;

pub use env::{Env, FILE_NAME, MAX_FILE_BYTES, system};
pub use error::{Error, Result};
pub use key::{DEFAULT_PREFIX, MAX_KEY_BYTES, MAX_VALUE_BYTES};
