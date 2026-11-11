# Changelog

All notable changes to `env-lib` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [1.0.1] - 2026-10-26

### Fixed

- The documentation builds on docs.rs again. The crate root asked nightly
  rustdoc for the `doc_auto_cfg` feature, which nightly has since folded
  into `doc_cfg` and removed, so the docs.rs build of 1.0.0 failed
  (`E0557`). It now asks for `doc_cfg`. No code change.

---

## [1.0.0] - 2026-10-13

Initial stable release. The public API is frozen for 1.x under
[`docs/STABILITY-1.0.md`](docs/STABILITY-1.0.md).

### Added

- `Env`, an application's own environment variables, optionally backed by
  a `.env` file. The system environment is read as a fallback but never
  copied into an `Env` or saved.
- Constructors: `Env::new()`; `Env::open(path)`, which loads the file or
  starts empty when it does not exist and expands a leading `~`;
  `Env::for_app(&path_lib::App)`, which opens `.env` (`FILE_NAME`) in the
  application's config directory; and `Env::from_text(text)`.
- Reads: `get` (the `Env`'s own value, else `<prefix><key>` from the
  process environment), `get_or`, `parse::<T: FromStr>` (`Ok(None)` when
  unset), `flag` (`true`/`1`/`yes`/`on` and `false`/`0`/`no`/`off`/empty,
  ignoring case), `contains`, `len`, `is_empty`, `iter` (file order, then
  insertion order), `is_dirty`, `file`, and `prefix`.
- Writes: `set`, `remove`, and `clear`. Names must be ASCII letters,
  digits, and `_`, not starting with a digit, at most 255 bytes
  (`MAX_KEY_BYTES`). Values may not contain NUL and are limited to
  32,767 bytes (`MAX_VALUE_BYTES`), the Windows per-variable limit.
- Persistence: `save`, `save_to(path)`, and `reload`. `save` re-reads
  the file and applies only the variables set or removed this session,
  so hand edits made while the program runs are kept and loaded into the
  `Env`; the program's value wins when both changed the same variable.
  `save_to` merges every variable into another file, keeping that file's
  other variables. Saves keep comments, blank lines, ordering, CRLF line
  endings, and the exact text of untouched entries; update changed values
  in place, keeping inline comments; drop removed variables; collapse
  duplicates to the last occurrence; and append new variables. Writes are atomic through `path_lib::write_atomic`, missing
  directories are created, and a new file is created with mode `0600` on
  Unix. Files over 1 MiB (`MAX_FILE_BYTES`) are rejected.
- The `.env` parser: `#` comments, `NAME=value` with optional spaces
  around `=`, `export NAME=value`, double quotes with `\n \r \t \" \\ \$`
  escapes, literal single quotes, multi-line quoted values, empty values,
  and a leading byte order mark. `${VAR}` interpolation is deliberately
  not performed. Malformed lines fail the load with their 1-based line
  number.
- Namespace prefix: `with_prefix`, defaulting to `DEFAULT_PREFIX`
  (`JG_`); an empty prefix falls back to the default. Names are plain in
  code and in the file, and a prefixed name addresses the same variable.
- `apply_to(&mut Command)` sets the prefixed variables on a child process
  and removes ones deleted since the last export. `unsafe fn export`
  writes them into the current process environment, under the same
  contract as `std::env::set_var`.
- `system(name)` reads a system environment variable exactly as named.
- `Error` (`#[non_exhaustive]`): `InvalidKey`, `InvalidValue`,
  `InvalidPrefix`, `Syntax`, `Parse`, `NoFile`, `TooLarge`, `Path`, and
  `Io`, and the `Result<T>` alias. No error and no `Debug` output contains
  a variable's value.
- Unit, integration, and property tests; criterion benchmarks; CI for
  Linux, macOS, and Windows on stable and MSRV 1.85.

[Unreleased]: https://github.com/jamesgober/env-lib/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/jamesgober/env-lib/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/jamesgober/env-lib/releases/tag/v1.0.0
