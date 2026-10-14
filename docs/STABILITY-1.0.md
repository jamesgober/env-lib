<h1 align="center">
    <b>env-lib</b><br>
    <sub><sup>STABILITY CONTRACT 1.0</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./API.md" title="API Reference"><b>API</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./PERFORMANCE.md" title="Performance"><b>PERFORMANCE</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./GUIDELINES.md" title="Developer Guidelines"><b>GUIDELINES</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <span>STABILITY</span>
    </sup>
</div>
<br>

This document is the contract `env-lib 1.0.0` ships under. It lists what is frozen for every 1.x release, what may still change in a minor or patch release, and how the contract itself changes.

> **Status:** In effect as of **1.0.0** and covers every 1.x release. Anything here that later needs to break requires a major version (2.0.0).

The signatures and behaviour of each item are documented in the [API reference](./API.md) and on [docs.rs](https://docs.rs/env-lib). Measured performance is in [PERFORMANCE.md](./PERFORMANCE.md).

---

## What 1.x Guarantees

### Public API surface

The items re-exported from the crate root are the complete public API. Their signatures and documented semantics do not change incompatibly within 1.x. The types are `Send + Sync`, and none of the functions panic.

#### Types and constants

| Item | Kind | Notes |
|---|---|---|
| `Env` | struct | `Debug + Clone + Default + Send + Sync`; `Debug` never shows values |
| `Error` | enum | `#[non_exhaustive]`; `InvalidKey`, `InvalidValue`, `InvalidPrefix`, `Syntax`, `Parse`, `NoFile`, `TooLarge`, `Path`, `Io` |
| `Result<T>` | type | `std::result::Result<T, Error>` |
| `DEFAULT_PREFIX` | const | `"JG_"` |
| `FILE_NAME` | const | `".env"` |
| `MAX_KEY_BYTES` | const | `255` |
| `MAX_VALUE_BYTES` | const | `32_767` |
| `MAX_FILE_BYTES` | const | `1_048_576` (1 MiB) |
| `system` | fn | reads a system variable exactly as named |

The values of the constants are part of the contract. The fields of each `Error` variant, the `From<path_lib::Error>` conversion, and `std::error::Error::source` returning the underlying `io::Error` or `path_lib::Error` are part of it too.

#### `Env` methods

| Method | Notes |
|---|---|
| `Env::new()` | empty, not tied to a file |
| `Env::open(path)` | loads `path`, or starts empty if it does not exist; tied to `path`; leading `~` expanded |
| `Env::for_app(&app)` | `open` on `FILE_NAME` in the `path_lib::App` config directory |
| `Env::from_text(text)` | parses text, not tied to a file |
| `Env::with_prefix(self, prefix)` | sets the process-environment prefix |
| `Env::prefix()`, `Env::file()` | what the `Env` was built with |
| `Env::get(key)`, `Env::get_or(key, default)` | own value, else `<prefix><key>` from the process environment |
| `Env::parse::<T>(key)`, `Env::flag(key)` | `Ok(None)` when unset |
| `Env::contains(key)`, `Env::len()`, `Env::is_empty()`, `Env::iter()` | the `Env`'s own variables only |
| `Env::is_dirty()` | unsaved changes |
| `Env::set(key, value)`, `Env::remove(key)`, `Env::clear()` | in-memory changes |
| `Env::save()`, `Env::save_to(path)`, `Env::reload()` | persistence |
| `Env::apply_to(&mut Command)` | safe hand-off to a child process |
| `unsafe Env::export()` | writes the current process environment |

### Names and the prefix

- **Naming rules.** A name is an ASCII letter or `_` followed by ASCII letters, digits, or `_`, at most `MAX_KEY_BYTES` bytes. The same rules apply to names in code, names in the file, and prefixes. Changing what is accepted is a major-version change.
- **Plain names.** Names are plain in code and in the file. The prefix is applied only to process-environment copies: by `apply_to`, by `export`, and by the fallback in `get`. A name passed with the prefix (`JG_PORT`) addresses the same variable as the plain name (`PORT`).
- **Default prefix.** The default prefix is `JG_`, and an empty prefix falls back to it. A prefix that is not a valid name fails with `Error::InvalidPrefix`.
- **Values.** Any text without NUL, up to `MAX_VALUE_BYTES` bytes, is accepted by `set` and read back unchanged after a save and a load.

### The file format

The syntax described in the [API reference](./API.md#the-file-format) is normative. Every file that loads under 1.0.0 loads with the same variables and values under every 1.x release. A minor release may accept syntax that 1.0.0 rejects only if that cannot change the meaning of any file that loads today. The format includes:

- `#` comments and blank lines; `NAME=value` with optional spaces or tabs around `=`; `export NAME=value`;
- unquoted values ending at the end of the line or at a `#` that starts the value or follows whitespace, trimmed;
- double-quoted values with the escapes `\n`, `\r`, `\t`, `\"`, `\\`, `\$`, other backslash sequences kept as written, and values spanning lines;
- single-quoted literal values, spanning lines;
- empty values; a leading UTF-8 byte order mark ignored; `\n` and `\r\n` line endings;
- no interpolation of `${VAR}`, `$VAR`, or `%VAR%`;
- malformed lines reported as errors with their 1-based line number, never skipped;
- files over `MAX_FILE_BYTES` and invalid UTF-8 rejected.

### Behavioural guarantees

- **The system environment is never copied in.** An `Env` holds only variables set in code and variables from its file. Nothing read from the process environment is stored or saved.
- **Saves apply only this session's changes.** `save` reads the file from disk again and applies only the variables set or removed since the last load or save. Everything else in the file as it is on disk is kept, including variables, values, deletions, and comments edited by hand while the program ran. When both changed the same variable, the program's value wins; a variable the program removed is removed even if the file changed it meanwhile. Afterwards the `Env` holds the merged result.
- **`save_to` merges.** Saving to a file other than the `Env`'s own writes every variable the `Env` holds into it and removes the variables removed this session; that file's other variables, comments, and layout are kept, and the `Env` then holds the merged set.
- **Lossless saves.** A save keeps comments, blank lines, ordering, line endings (`\r\n` stays `\r\n`), and the exact text of every entry the program did not change. Changed values are updated in place, keeping `export` and any inline comment with the whitespace before it; removed variables are dropped; duplicate entries collapse to the last occurrence; new variables are appended. Opening and saving a file without changes leaves its bytes unchanged, apart from collapsing duplicates, dropping a byte order mark, adding a missing final line ending, and normalising mixed line endings to `\r\n`.
- **Atomic writes.** A save goes through `path_lib::write_atomic`: readers see the old file or the complete new one, and a failed save leaves the existing file untouched. Missing directories are created. The read at the start of a save and the final rename are not locked against other processes, so a write by another process between the two is overwritten.
- **Private new files.** On Unix a file created by a save has mode `0600`. An existing file keeps its permissions.
- **Values never appear in output.** No `Error` and no `Debug` output contains a variable's value. Errors name the variable, the file, and the line.
- **Safe hand-off.** `apply_to` changes only the `Command` it is given. Both `apply_to` and `export` remove the prefixed names of variables deleted since the last `export`, whether they were removed in code, by `reload`, or by a hand edit picked up by a save.
- **Iteration order.** `iter` yields variables in file order and then in the order they were first set.

### Dependencies and toolchain

| Property | Value |
|---|---|
| **Runtime dependencies** | `path-lib` 1.x only |
| **Edition** | 2024 |
| **MSRV** | Rust 1.85, frozen for 1.x |
| **License** | Apache-2.0 OR MIT |
| **Platforms** | Linux, macOS, Windows; FreeBSD and other Unix systems follow the Linux behaviour |

Raising the MSRV or adding a runtime dependency is a major-version change. The crate has no feature flags. Dev-dependencies may change freely.

---

## What Can Change in 1.x

### Additions in minor releases

- New `Error` variants. `Error` is `#[non_exhaustive]`; match on it with a wildcard arm.
- New functions, constants, and methods, provided existing ones keep their signatures and semantics.
- New trait implementations on existing types.

### Not covered by the contract

- **Error message text.** The `Display` output of `Error`, including the `reason` strings of `InvalidKey`, `InvalidValue`, `InvalidPrefix`, and `Syntax`, may be reworded. The variant, its fields, and the line number are the stable part; match on those, not on strings.
- **The `op` text of `Error::Io`.** It names the step that failed for people reading the message, not for matching.
- **The quoting style of newly written entries.** Which values are quoted, and how, may change, as long as every value written still reads back identically. Entries the program did not change are never re-rendered.
- **The `Debug` format of `Env`,** except that it never includes values.
- **Performance.** The numbers in [PERFORMANCE.md](./PERFORMANCE.md) are measurements, not guarantees, and may change in any release.
- **Internal structure:** module layout and private types.

### Outside the 1.x scope

Layered files (`.env` plus `.env.local`), file watching and automatic reload, encrypted values, `${VAR}` interpolation, and per-variable prefixes are not part of 1.0. Interpolation is excluded by design and will not be added to the default parser in 1.x, since it would change the values of existing files. The others may be added in a minor release only as new, opt-in items.

---

## Versioning Policy

| Bump | Triggered by |
|---|---|
| Major | Any incompatible change to an item listed above; a change to the naming rules, the accepted file syntax, the default prefix, or the save guarantees; weakening a behavioural guarantee; raising the MSRV; adding a runtime dependency |
| Minor | New items or `Error` variants; performance improvements |
| Patch | Bug fixes, documentation, internal changes with no behavioural effect |

### Deprecation

An item is deprecated with `#[deprecated(since = "1.Y.0", note = "...")]` in a minor release and removed no earlier than 2.0.0. The release notes describe the replacement.

---

## How the Contract Is Enforced

- **CI** runs `cargo fmt --check`, `cargo clippy -D warnings`, the full test suite, and `cargo doc` with `-D warnings` on Linux, macOS, and Windows, on stable and Rust 1.85.0, and type-checks FreeBSD.
- **Unit tests** pin the parser's accepted forms, its error reasons and line numbers, the writer's quoting, the save rules, the naming and prefix rules, and that no error or `Debug` output contains a value.
- **Property tests** check that any set of variables survives a save and a load unchanged, that the parser never panics on arbitrary text, and that opening and saving without changes is byte-identical.
- **Integration tests** cover sessions across restarts, hand edits kept verbatim, hand edits made while the program runs surviving a save, removals winning over concurrent edits, inline comments kept on changed values, merging into another file with `save_to`, `reload`, CRLF files, missing directories, oversized and non-UTF-8 files, and on Unix the `0600` mode of new files. `export` is tested in its own binary so no other test touches the process environment at the same time.
- **`cargo audit`** and **`cargo deny`** run on every push.

---

## Changing This Document

When an item on the frozen list needs to change:

1. The change is a major version. It does not land on a 1.x branch.
2. Open an issue that states what would change and why.
3. Update this document on the 2.0 branch.
4. Where possible, deprecate the old item in a 1.x minor release first.

<br>

<div align="center">
  <sup>env-lib 1.0.0 &mdash; COPYRIGHT <small>&copy;</small> 2026 <strong>JAMES GOBER.</strong> Apache-2.0 OR MIT.</sup>
</div>
