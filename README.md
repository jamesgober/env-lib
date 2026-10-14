<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br><b>env-lib</b>
    <br><sub><sup>APPLICATION ENVIRONMENT VARIABLES FOR RUST</sup></sub>
</h1>

<p align="center">
    <a href="https://crates.io/crates/env-lib"><img src="https://img.shields.io/crates/v/env-lib.svg" alt="Crates.io"></a>
    <a href="https://crates.io/crates/env-lib" alt="Download"><img alt="Crates.io Downloads" src="https://img.shields.io/crates/d/env-lib?color=%230099ff"></a>
    <a href="https://docs.rs/env-lib"><img src="https://docs.rs/env-lib/badge.svg" alt="Documentation"></a>
    <a href="https://github.com/jamesgober/env-lib/actions"><img alt="GitHub CI" src="https://github.com/jamesgober/env-lib/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md" title="MSRV"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</p>

<p align="center">
    <b>Your Application's Variables, in a File That Survives Hand Edits</b>
    <br>
    <i>Set, read, and remove in code; save to <code>.env</code>; load it next run. No third-party dependencies.</i>
</p>

<p>
    <strong>env-lib</strong> manages the environment variables that belong to your application. Set, read, and remove them in code, save them to a <code>.env</code> file, and the next run loads exactly what the last one saved. Most <code>.env</code> crates read a file once at startup and copy it into the process environment; <strong>env-lib</strong> treats the file as the application's own store, one that the program writes as well as reads.
</p>

<p>
    Saving is lossless. A save reads the file again and applies only what the program changed in this session, so a file that a person edits by hand and a program edits in code can be the same file, even while the program is running. Comments, blank lines, ordering, line endings, and the exact text of every entry the program did not change are kept. Changed values are updated in place, keeping their inline comments; removed variables are dropped; new ones are appended. The write is atomic, and a newly created file is readable only by its owner on Unix.
</p>

<p>
    The system environment is read but never copied in. Names are plain in code and in the file (<code>PORT</code>); only the copies placed in a process environment carry a namespace prefix (<code>JG_PORT</code> by default), so application variables never collide with system ones. Reads fall back to the process environment under that prefix, which picks up values set by a parent process. Values never appear in <code>Debug</code> output or in error messages.
</p>

<p>
    The API is stable. Version 1.0 freezes the public items, the file format, and the guarantees above for every 1.x release; the terms are in the <a href="docs/STABILITY-1.0.md">stability contract</a>.
</p>

## Features

### Variables in Code
- **`set`, `remove`, `clear`** &mdash; names are checked when set: ASCII letters, digits, and `_`, not starting with a digit
- **`get`, `get_or`** &mdash; the `Env`'s own value, else the process environment under the prefix
- **`parse::<T>`** &mdash; any `FromStr` type; `Ok(None)` when unset, an error naming the variable when it does not parse
- **`flag`** &mdash; `true`/`1`/`yes`/`on` and `false`/`0`/`no`/`off`, any case
- **`system(name)`** &mdash; reads a system variable exactly as named, without the prefix

### The `.env` File
- **`Env::open(path)`** &mdash; loads the file, or starts empty if it does not exist yet; `~` is expanded
- **`Env::for_app(&app)`** &mdash; `.env` in the application's config directory, resolved by [`path-lib`](https://crates.io/crates/path-lib)
- **`save`, `save_to`, `reload`** &mdash; persist changes, or pick up edits made outside the program
- **Hand edits survive** &mdash; a save re-reads the file and applies only this session's changes, so edits made while the program runs are kept
- **Lossless saves** &mdash; comments, blank lines, order, CRLF endings, and untouched entries kept byte for byte; inline comments kept on changed values
- **Familiar syntax** &mdash; `#` comments, `export NAME=value`, single and double quotes, multi-line values
- **No interpolation** &mdash; `${OTHER}` is kept as written, so a file cannot pull other environment state into a value
- **Errors with line numbers** &mdash; a malformed line fails the load with its line number; nothing is skipped silently

### Handing Variables to Other Code
- **`apply_to(&mut Command)`** &mdash; sets the prefixed variables on a child process; safe
- **`export()`** &mdash; writes them into the current process environment for code that reads `std::env` directly; `unsafe`, for use at startup
- **`with_prefix("HIVE_")`** &mdash; one namespace prefix for every exported variable; `JG_` by default

### Secrets
- **Never printed** &mdash; `Debug` lists names only; errors name the variable, file, and line, never the value
- **Owner-only files** &mdash; a newly created file is `0600` on Unix; an existing file keeps its permissions
- **Atomic writes** &mdash; readers see the old file or the complete new one, never a partial write

<hr>

## Installation

```toml
[dependencies]
env-lib = "1.0"
```

`Env::for_app` takes a `path_lib::App`. To use it, add `path-lib = "1.0"` as well; `env-lib` already depends on it, so no additional code is compiled.

<hr>

## Quick Start

```rust
use env_lib::Env;

// The file is ~/.config/HiveDB/.env on Linux, %APPDATA%\HiveDB\.env on Windows.
let app = path_lib::App::new("HiveDB")?;
let mut env = Env::for_app(&app)?;            // a missing file is an empty Env

env.set("PORT", "7000")?;
env.set("DB_URL", "postgres://localhost/hive")?;
env.remove("LEGACY_MODE");
env.save()?;                                  // comments and hand edits are kept

// Next run, or after someone edits the file by hand:
let env = Env::for_app(&app)?;
let port: u16 = env.parse("PORT")?.unwrap_or(8080);
let debug = env.flag("DEBUG")?.unwrap_or(false);
let level = env.get_or("LOG_LEVEL", "info");
# Ok::<(), env_lib::Error>(())
```

### The File

```text
# HiveDB settings
PORT=7000
DB_URL=postgres://localhost/hive

# added by hand; kept exactly as written on every save
export LOG_LEVEL = debug   # temporary
GREETING="Hello,\nworld"
```

### Child Processes

```rust
use std::process::Command;

let mut env = env_lib::Env::from_text("PORT=7000\n")?.with_prefix("HIVE_")?;
env.set("WORKERS", "8")?;

let mut cmd = Command::new("hive-worker");
env.apply_to(&mut cmd);                       // HIVE_PORT=7000, HIVE_WORKERS=8
# Ok::<(), env_lib::Error>(())
```

The parent's own environment is not touched. For third-party code that reads `std::env` directly, `export` writes the same variables into the current process; it is `unsafe` for the same reason `std::env::set_var` is, and belongs at the start of `main`, before any thread is spawned.

<hr>

## Documentation

- [API reference](docs/API.md) &mdash; every public item, the file format, and the save rules.
- [Stability contract](docs/STABILITY-1.0.md) &mdash; what 1.x guarantees and what may still change.
- [Performance](docs/PERFORMANCE.md) &mdash; benchmark methodology and results.
- [Developer guidelines](docs/GUIDELINES.md) &mdash; the engineering bar this project is built to.
- [Release notes](docs/release/) &mdash; what shipped, when, and why.

<br>

## Contributing

Contributions are welcome under the project's dual license. Before opening a pull request, please make sure:

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy --all-targets --all-features -- -D warnings` is clean.
3. `cargo test --all-features` passes.
4. New public items include documentation and at least one example.
5. The [API reference](docs/API.md) and [CHANGELOG](CHANGELOG.md) are updated alongside code changes.

<br>

<!-- LICENSE
############################################# -->
<div id="license">
    <h2>License</h2>
    <p><b>env-lib</b> is <b>dual-licensed</b> under either of:</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; see <a href="./LICENSE-APACHE" title="Apache-2.0 License">LICENSE-APACHE</a> or <a href="http://www.apache.org/licenses/LICENSE-2.0" title="Apache-2.0 License" target="_blank">apache.org/licenses/LICENSE-2.0</a></li>
        <li><b>MIT License</b> &mdash; see <a href="./LICENSE-MIT" title="MIT License">LICENSE-MIT</a> or <a href="https://opensource.org/licenses/MIT" title="MIT License" target="_blank">opensource.org/licenses/MIT</a></li>
    </ul>
    <p>at your option.</p>
    <br>
    <h3>Contribution</h3>
    <p>Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in <b>env-lib</b> by you, as defined in the <b>Apache-2.0 license</b>, shall be <b>dual-licensed</b> as above, without any additional terms or conditions.</p>
    <p>Unless required by applicable law or agreed to in writing, software distributed under the Licenses is distributed on an <b>"AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND</b>, either express or implied.</p>
</div>

<!-- FOOT COPYRIGHT
################################################# -->
<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>JAMES GOBER.</strong></sup>
</div>
