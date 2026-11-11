<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br><b>env-lib</b><br>
    <sub><sup>API REFERENCE</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <span>API</span>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./PERFORMANCE.md" title="Performance"><b>PERFORMANCE</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./GUIDELINES.md" title="Developer Guidelines"><b>GUIDELINES</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./STABILITY-1.0.md" title="Stability Contract"><b>STABILITY</b></a>
    </sup>
</div>
<br>

> **Status:** This reference tracks the public surface of `env-lib` **1.0.1**. Everything documented here is frozen for 1.x under the [stability contract](./STABILITY-1.0.md).

## Table of Contents

- [Env](#env)
  - [Constructors](#constructors)
  - [Names and the prefix](#names-and-the-prefix)
  - [Reading](#reading)
  - [Writing](#writing)
  - [Saving and reloading](#saving-and-reloading)
  - [Handing variables to other code](#handing-variables-to-other-code)
  - [Inspecting](#inspecting)
  - [Sharing between threads](#sharing-between-threads)
- [`system`](#system)
- [The File Format](#the-file-format)
  - [Accepted syntax](#accepted-syntax)
  - [What a save keeps](#what-a-save-keeps)
  - [How new values are written](#how-new-values-are-written)
- [Limits](#limits)
- [Errors](#errors)
- [Design Notes](#design-notes)
- [Not Included](#not-included)

<br>

## Env

`Env` holds one application's own environment variables: the ones its code sets and the ones in its `.env` file. The system environment is never copied into it and never saved. An `Env` may be tied to a file, which `save` writes and `reload` reads again.

`Env` is `Debug + Clone + Default + Send + Sync`. `Default` is `Env::new()`.

### Constructors

| Function | File | Fails when |
|---|---|---|
| `Env::new()` | None | Never |
| `Env::open(path)` | `path` | The file exists but is malformed, too large, or unreadable |
| `Env::for_app(&app)` | `.env` in the app's config directory | As for `open` |
| `Env::from_text(text)` | None | The text is malformed |

```rust
use env_lib::Env;

let dir = std::env::temp_dir().join("env-lib-api-constructors");
let file = dir.join(".env");

let empty = Env::new();
let parsed = Env::from_text("PORT=7000\n")?;
let opened = Env::open(&file)?;               // the file does not exist yet
assert!(opened.is_empty());
assert_eq!(opened.file(), Some(file.as_path()));
# std::fs::remove_dir_all(&dir).ok();
# Ok::<(), env_lib::Error>(())
```

`Env::open` loads the file if it exists and starts empty if it does not. Either way the `Env` is tied to `path`, so the first `save` creates the file. A leading `~` is expanded to the home directory through `path_lib::expand_home`; a path starting with `~` fails with `Error::Path` when it cannot be expanded, either because there is no home directory or because it names another user (`~name`).

`Env::for_app` opens `FILE_NAME` (`.env`) in the config directory of a [`path_lib::App`](https://docs.rs/path-lib): `~/.config/<app>/.env` on Linux, `~/Library/Preferences/<app>/.env` on macOS, and `%APPDATA%\<app>\.env` on Windows for user scope. A global or portable `App` gives the matching directory for that scope.

```rust
let app = path_lib::App::new("HiveDB")?;
let env = env_lib::Env::for_app(&app)?;
# Ok::<(), env_lib::Error>(())
```

`Env::new` and `Env::from_text` are not tied to a file. Write them out with `save_to`, which ties them to the path it writes.

When a variable appears more than once in a file, the last occurrence wins, and the next save keeps only that line.

### Names and the prefix

Variable names follow the portable shell rules: an ASCII letter or `_`, then ASCII letters, digits, or `_`, at most `MAX_KEY_BYTES` (255) bytes. Dots, dashes, spaces, and non-ASCII letters are rejected rather than changed, because they cannot be used from a shell or are not portable to Windows.

Names are plain in code and in the file: `PORT`, not `JG_PORT`. The namespace prefix is added only when a variable is placed in a process environment, by `apply_to` or `export`, and when `get` falls back to the process environment.

```rust
let mut env = env_lib::Env::new();
assert_eq!(env.prefix(), "JG_");                       // DEFAULT_PREFIX

env.set("JG_PORT", "7000")?;                           // same variable as PORT
assert_eq!(env.get("PORT").as_deref(), Some("7000"));
assert_eq!(env.iter().collect::<Vec<_>>(), [("PORT", "7000")]);

let env = env.with_prefix("HIVE_")?;
assert_eq!(env.prefix(), "HIVE_");
assert_eq!(env_lib::Env::new().with_prefix("")?.prefix(), "JG_");
assert!(env_lib::Env::new().with_prefix("JG.").is_err());
# Ok::<(), env_lib::Error>(())
```

| Method | Does |
|---|---|
| `with_prefix(self, prefix)` | Sets the prefix; returns the `Env` for chaining |
| `prefix()` | The current prefix, `JG_` unless changed |

The default prefix is `DEFAULT_PREFIX`, `"JG_"`. An empty prefix means the default. Any other prefix must itself be a valid name, or `with_prefix` fails with `Error::InvalidPrefix`; `JG.` is rejected because a dot cannot appear in a shell variable name. The prefix is joined to the name as written, so include the separator: `HIVE_` gives `HIVE_PORT`, while `HIVE` gives `HIVEPORT`.

Every method that takes a name accepts it with or without the prefix: with the prefix `JG_`, both `PORT` and `JG_PORT` address the variable `PORT`. A name equal to the prefix alone (`JG_`) is a name of its own. Write names in the file without the prefix; the prefix is removed from names passed to methods, not from names read from the file.

### Reading

| Method | Returns | Reads the process environment |
|---|---|---|
| `get(key)` | `Option<Cow<str>>` | Yes, as a fallback |
| `get_or(key, default)` | `Cow<str>` | Yes, as a fallback |
| `parse::<T>(key)` | `Result<Option<T>>` | Yes, as a fallback |
| `flag(key)` | `Result<Option<bool>>` | Yes, as a fallback |
| `contains(key)` | `bool` | No |

```rust
let env = env_lib::Env::from_text("PORT=7000\nDEBUG=on\nWORKERS=eight\n")?;

assert_eq!(env.get("PORT").as_deref(), Some("7000"));
assert_eq!(env.get_or("LOG_LEVEL", "info"), "info");
assert_eq!(env.parse::<u16>("PORT")?, Some(7000));
assert_eq!(env.parse::<u16>("TIMEOUT")?, None);
assert!(env.parse::<usize>("WORKERS").is_err());
assert_eq!(env.flag("DEBUG")?, Some(true));
assert!(env.contains("PORT"));
# Ok::<(), env_lib::Error>(())
```

`get` returns this `Env`'s own value when it has one, borrowed. Otherwise it reads `<prefix><key>` from the process environment (`JG_PORT`), which finds values set by a parent process or by an earlier `export`, and returns an owned copy. It returns `None` when neither has the variable, when the process value is not valid Unicode, or when the name is not valid; an invalid name never reaches the operating system. `contains`, `len`, `iter`, and `is_empty` see only the `Env`'s own variables.

`get_or` returns `default` when `get` would return `None`. Call `.into_owned()` on the result for a `String`.

`parse` trims surrounding whitespace and parses with `FromStr`. It returns `Ok(None)` when the variable is not set and `Error::Parse` when it is set but does not parse. The error names the variable and the target type, not the value.

`flag` reads an on/off switch. Ignoring case and surrounding whitespace, `true`, `1`, `yes`, and `on` are `true`; `false`, `0`, `no`, `off`, and the empty string are `false`. Anything else is `Error::Parse`. It returns `Ok(None)` when the variable is not set.

### Writing

| Method | Does | Fails with |
|---|---|---|
| `set(key, value)` | Adds or updates a variable | `InvalidKey`, `InvalidValue` |
| `remove(key)` | Removes a variable; returns whether it was present | Never |
| `clear()` | Removes every variable | Never |

```rust
let mut env = env_lib::Env::new();
env.set("PORT", "7000")?;
env.set("GREETING", String::from("hello, world"))?;
assert!(env.remove("PORT"));
assert!(!env.remove("PORT"));
assert!(env.set("db.url", "x").is_err());              // dots are not allowed
assert!(env.set("TOKEN", "a\0b").is_err());            // nor is NUL in a value
# Ok::<(), env_lib::Error>(())
```

`set` takes any `Into<String>` value. A value may contain anything except NUL, including newlines, quotes, `$`, and `#`, up to `MAX_VALUE_BYTES` (32,767) bytes. Setting a variable to the value it already has is not a change and does not mark the `Env` as dirty.

`remove` and `clear` change only the `Env`. The next save removes those variables from the file, and the next `apply_to` or `export` removes them from the target environment. After `clear`, the next save removes every variable the `Env` held; the file keeps its comments, blank lines, and any variables added to it by hand after it was loaded.

Changes are held in memory until `save`. The `Env` records which variables were set or removed since it was loaded or last saved, and that record is what a save applies to the file. `is_dirty` reports whether there are any such changes. Setting a variable back to the value it was loaded with still counts as a change.

### Saving and reloading

| Method | Does | Fails with |
|---|---|---|
| `save()` | Applies this session's changes to the `Env`'s file as it is on disk | `NoFile` if not tied to a file; otherwise as `save_to` |
| `save_to(path)` | Writes every variable into `path`, merging with what is there, and ties the `Env` to it | `Syntax` or `TooLarge` if the file on disk is malformed or too large; `Path`, `Io` |
| `reload()` | Discards unsaved changes and reads the file again | `NoFile`; otherwise as `open` |

```rust
use env_lib::Env;

let dir = std::env::temp_dir().join("env-lib-api-save");
let file = dir.join("config").join(".env");            // directories are created

let mut env = Env::open(&file)?;
env.set("PORT", "7000")?;
env.save()?;
assert!(!env.is_dirty());

// While the program runs, someone edits the file by hand.
std::fs::write(&file, "# edited by hand\nPORT=7000\nLOG_LEVEL=debug\n").unwrap();
env.set("WORKERS", "8")?;
env.save()?;                                           // the hand edit is kept
assert_eq!(env.get("LOG_LEVEL").as_deref(), Some("debug"));
assert_eq!(
    std::fs::read_to_string(&file).unwrap(),
    "# edited by hand\nPORT=7000\nLOG_LEVEL=debug\nWORKERS=8\n",
);

std::fs::write(&file, "PORT=7001\n").unwrap();
env.set("UNSAVED", "x")?;
env.reload()?;                                         // discards UNSAVED
assert_eq!(env.get("PORT").as_deref(), Some("7001"));
assert!(!env.contains("UNSAVED"));
# std::fs::remove_dir_all(&dir).ok();
# Ok::<(), env_lib::Error>(())
```

`save` reads the file from disk again and applies this session's changes to what it finds: variables set since the last load or save are updated in place or appended, and variables removed are dropped. Everything else is kept as it is on disk, including edits made by hand while the program was running, whether they added, changed, or deleted variables or comments. When the file and the program both changed the same variable, the program's value wins, and a variable the program removed is removed even if the file changed it in the meantime. After the save the `Env` holds the merged result, so its values and order match the file. A save with no changes still reads and rewrites the file, so it also picks up hand edits without discarding anything. The [save rules](#what-a-save-keeps) list exactly what is kept.

If the file was deleted after it was loaded, the save recreates it with only this session's changes. Variables that are no longer in the file after a save, because they were deleted by hand, are removed from the target environment by the next `apply_to` or `export`, as after `reload`. If the file on disk has become malformed, the save fails with `Error::Syntax` and writes nothing; the `Env` keeps its changes, so fix the file and save again.

The save writes the whole file through `path_lib::write_atomic`, so a reader sees either the old file or the complete new one, never a partial write. Missing directories are created. On Unix a newly created file is given mode `0600` before anything is written to it, since `.env` files often hold secrets; an existing file keeps its permissions. On Windows the file inherits the directory's access control list, and the per-user config directory is already private to the user. A failed save leaves the existing file untouched.

`save_to(path)` with the `Env`'s own file is the same as `save`. With any other path, every variable the `Env` holds is written into that file and variables removed this session are removed from it; the file's other variables, comments, and layout are kept. The `Env` is then tied to `path`, so later calls to `save` and `reload` use it, and it holds the merged set, including the variables that were already in that file.

`reload` reads the file again and replaces the `Env`'s variables with what it finds, discarding unsaved changes. The prefix is kept. A file that no longer exists reloads as empty. Variables that the reload removed are remembered, so the next `apply_to` or `export` removes them from the target environment.

Two processes that save the same file each apply only their own changes to its latest contents, so changes to different variables are all kept, and for the same variable the last save wins. Reading the file and renaming the new one into place are not locked against other processes: a write by another process that lands between the two is overwritten. The window covers only the merge and the write, but simultaneous saves from several processes are not coordinated.

### Handing variables to other code

| Method | Target | Safe |
|---|---|---|
| `get`, `get_or`, `parse`, `flag` | Your own code | Yes |
| `apply_to(&mut Command)` | A child process | Yes |
| `export()` | The current process environment | No (`unsafe fn`) |

Prefer reading through the `Env`. Use `apply_to` to pass the variables to a program you start, and `export` only for code you do not control that reads `std::env` itself.

```rust
use std::process::Command;

let mut env = env_lib::Env::from_text("PORT=7000\nOLD=1\n")?;
env.remove("OLD");

let mut cmd = Command::new("hive-worker");
env.apply_to(&mut cmd).arg("--serve");                 // returns &mut Command
let vars: Vec<_> = cmd.get_envs().collect();
assert!(vars.contains(&("JG_PORT".as_ref(), Some("7000".as_ref()))));
assert!(vars.contains(&("JG_OLD".as_ref(), None)));    // removed in the child
# Ok::<(), env_lib::Error>(())
```

`apply_to` sets every variable, prefixed, on the `Command`, and removes from it the prefixed names of variables deleted since the last `export`. It changes only the `Command`, never the parent's environment, and returns the `Command` for chaining.

`export` sets every variable, prefixed, in the current process environment, and removes the prefixed names of variables deleted since the last `export`. Afterwards `std::env::var("JG_PORT")` sees the value, and so does `get` on any `Env` with the same prefix.

```rust,no_run
fn main() -> Result<(), env_lib::Error> {
    let app = path_lib::App::new("HiveDB")?;
    let mut env = env_lib::Env::for_app(&app)?;

    // SAFETY: the first thing main does; no other thread exists yet.
    unsafe { env.export() };

    // ... start the runtime and spawn threads after this point.
    Ok(())
}
```

`export` is `unsafe` because changing the process environment while another thread reads or writes it is undefined behavior, including reads made indirectly by C functions such as `getenv`, which DNS resolution and time-zone handling call. The caller must guarantee that no other thread touches the environment for the duration of the call: call it at startup, before spawning threads or starting an async runtime. This is the same contract as `std::env::set_var` and `std::env::remove_var` in edition 2024. Names are validated and values contain no NUL, so `export` cannot panic.

### Inspecting

| Method | Returns |
|---|---|
| `len()`, `is_empty()` | How many variables the `Env` holds |
| `iter()` | `(&str, &str)` pairs, in file order and then in the order they were first set |
| `is_dirty()` | Whether there are changes not yet saved |
| `file()` | `Option<&Path>`, the file `save` writes |
| `prefix()` | The process-environment prefix |

`Debug` shows the file, the prefix, the variable names, and whether the `Env` is dirty. It never shows values.

```rust
let mut env = env_lib::Env::from_text("B=1\nA=2\n")?;
env.set("C", "3")?;
env.set("TOKEN", "s3cr3t")?;
assert_eq!(env.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["B", "A", "C", "TOKEN"]);
assert!(!format!("{env:?}").contains("s3cr3t"));
# Ok::<(), env_lib::Error>(())
```

### Sharing between threads

`Env` is `Send + Sync`, and every reading method takes `&self`, so many threads can read one `Env` at once. Changes take `&mut self`. To share an `Env` that is still being changed, put it behind a `RwLock`; for one that is loaded once and only read afterwards, a `OnceLock` static is enough.

```rust
use std::sync::OnceLock;
use env_lib::Env;

static ENV: OnceLock<Env> = OnceLock::new();

fn env() -> &'static Env {
    ENV.get_or_init(|| Env::from_text("PORT=7000\n").unwrap_or_default())
}

assert_eq!(env().get("PORT").as_deref(), Some("7000"));
```

<br>

## `system`

```rust
let name = if cfg!(windows) { "SystemRoot" } else { "PATH" };
let value: Option<String> = env_lib::system(name);
assert!(value.is_some());
```

`system(name)` reads a system environment variable exactly as named, with no prefix and no validation beyond what the operating system requires. It returns `None` when the variable is unset or not valid Unicode, and for a name that is empty or contains `=` or NUL. Use it for variables that belong to the system, such as `PATH` or `HOME`; `Env::get` is for the application's own variables.

<br>

## The File Format

### Accepted syntax

```text
# comment
NAME=value                 unquoted: ends at the end of the line or at " #"; trimmed
NAME = value               spaces and tabs around "=" are allowed
export NAME=value          "export " is accepted and kept on save
NAME="a\nb \"q\" \$x"      double quotes: escapes, may span lines
NAME='literal $x'          single quotes: no escapes, may span lines
NAME=                      empty value
```

| Form | Rules |
|---|---|
| Comments and blank lines | A line whose first non-blank character is `#`, or that is blank. Kept verbatim. |
| Names | The [naming rules](#names-and-the-prefix). Leading whitespace on the line is allowed. |
| `export` | `export` followed by a space or tab. `export=1` is a variable named `export`. |
| Unquoted values | Run to the end of the line, or to a `#` that starts the value or follows a space or tab. Surrounding whitespace is trimmed. `A=x#y` is `x#y`; `A=x #y` is `x`. |
| Double quotes | `\n`, `\r`, `\t`, `\"`, `\\`, and `\$` are escapes. Any other backslash sequence is kept as written, backslash included. The value may span lines. |
| Single quotes | Everything up to the closing `'` is literal, including backslashes. The value may span lines. |
| After a closing quote | Only whitespace or a `#` comment. |
| Encoding | UTF-8. A leading byte order mark is ignored. `\n` and `\r\n` line endings are both accepted. |

`${OTHER}`, `$OTHER`, and `%OTHER%` are not expanded; a value is exactly what the file says. See the [design notes](#design-notes).

A line that does not fit these rules is an error, never skipped. The load fails with `Error::Syntax`, which carries the file (when there is one), the 1-based line number, and the reason. For a quoted value that spans lines, the line number is the line where the entry starts.

| Reason | Example |
|---|---|
| `expected NAME=value` | `PORT 7000` |
| `invalid variable name` | `1PORT=7000`, `db.url=x` |
| `unterminated double-quoted value` | `A="open` with no closing quote anywhere below |
| `unterminated single-quoted value` | `A='open` |
| `unexpected text after the closing quote` | `A="x" y` |
| `value contains NUL or is too long` | A NUL byte, or a value over `MAX_VALUE_BYTES` |
| `not valid UTF-8` | Invalid UTF-8 on that line |

A file larger than `MAX_FILE_BYTES` (1 MiB) fails with `Error::TooLarge` before it is parsed.

### What a save keeps

A save starts from the file as it is on disk at that moment and applies only what the program changed:

- **Comments and blank lines** are written back exactly, wherever they are.
- **Entries the program did not change** keep their exact text as it is on disk: quoting, spacing, `export`, and inline comments, including edits made by hand while the program ran.
- **Changed values** are rewritten in place as `NAME=value`, keeping `export` and any inline comment together with the whitespace before it. Spacing around `=` and the original quoting are not kept; the new value is quoted only if it needs to be. Trailing whitespace with no comment after it is dropped.
- **Removed variables** are dropped.
- **Duplicates** collapse to one line, at the position of the last occurrence.
- **New variables** are appended at the end, in the order they were first set.
- **Line endings** are `\r\n` on every line if the file contains any `\r\n`; otherwise `\n`. A saved file always ends with a line ending, and a byte order mark is not written back.

Opening a file and saving it without changes produces the same bytes, except that duplicate entries collapse, a byte order mark is dropped, a missing final line ending is added, and mixed line endings become all `
`.

```rust
let dir = std::env::temp_dir().join("env-lib-api-lossless");
let file = dir.join(".env");
std::fs::create_dir_all(&dir).unwrap();
std::fs::write(&file, "# database\nexport DB_HOST = 'db.local'   # primary\nPORT=7000   # http listener\nOLD=1\n").unwrap();

let mut env = env_lib::Env::open(&file)?;
env.set("PORT", "7001")?;
env.remove("OLD");
env.set("WORKERS", "8")?;
env.save()?;

assert_eq!(
    std::fs::read_to_string(&file).unwrap(),
    "# database\nexport DB_HOST = 'db.local'   # primary\nPORT=7001   # http listener\nWORKERS=8\n",
);
# std::fs::remove_dir_all(&dir).ok();
# Ok::<(), env_lib::Error>(())
```

### How new values are written

New and changed entries are written unquoted when the value contains only ASCII letters, digits, and `_ - . / : @ , + % =`, including the empty value. Anything else is written in double quotes with `\`, `"`, `$`, newline, carriage return, and tab escaped. Escaping `$` keeps other `.env` tools that do interpolate double-quoted values from expanding it. Every value `set` accepts is read back identically by `open`.

```text
PORT=7000
DB_URL=postgres://localhost/hive
GREETING="hello, world"
PRICE="\$5"
MOTD="line one\nline two"
```

<br>

## Limits

| Constant | Value | Applies to |
|---|---|---|
| `MAX_KEY_BYTES` | `255` | Variable names, in bytes |
| `MAX_VALUE_BYTES` | `32_767` | Values, in bytes |
| `MAX_FILE_BYTES` | `1_048_576` (1 MiB) | `.env` files read by `open`, `reload`, `save`, and `save_to` |
| `DEFAULT_PREFIX` | `"JG_"` | The process-environment prefix when none is set |
| `FILE_NAME` | `".env"` | The file name `for_app` uses |

`MAX_VALUE_BYTES` matches the Windows limit of 32,767 UTF-16 units per variable. Counting bytes instead keeps every accepted value exportable on every platform.

<br>

## Errors

All fallible functions return `env_lib::Result<T>`, an alias for `std::result::Result<T, env_lib::Error>`.

| Variant | Meaning | What to do |
|---|---|---|
| `InvalidKey { key, reason }` | A name broke the naming rules | Fix the name; retrying fails the same way |
| `InvalidValue { key, reason }` | A value contains NUL or is over `MAX_VALUE_BYTES` | Fix the value |
| `InvalidPrefix { prefix, reason }` | `with_prefix` got an invalid prefix | Use letters, digits, and `_`, not starting with a digit |
| `Syntax { path, line, reason }` | A `.env` file or text could not be parsed | Fix the line and try again; nothing was loaded or written |
| `Parse { key, type_name }` | `parse` or `flag` got a value that does not parse | Fix the value, or handle the error with a default |
| `NoFile` | `save` or `reload` on an `Env` not tied to a file | Use `save_to`, or create the `Env` with `open` |
| `TooLarge { path }` | A file is over `MAX_FILE_BYTES` | The file is not a `.env` file this crate will load |
| `Path(path_lib::Error)` | Home expansion, directory resolution, or the atomic write failed | Usually permissions, disk space, or no home directory |
| `Io { op, path, source }` | The operating system reported an error | Usually permissions; `op` names the step, such as `"read"` |

No error contains a variable's value, because values are often secrets and errors end up in logs. Errors name the variable, the file, and the line instead. Names and prefixes echoed in an error are truncated to 64 characters.

`Error` implements `Display` and `std::error::Error`; `source()` returns the underlying `io::Error` or `path_lib::Error`. `From<path_lib::Error>` lets `?` convert errors from `path_lib::App::new` and other `path-lib` calls. `Error` is `#[non_exhaustive]`, so match it with a wildcard arm.

<br>

## Design Notes

**The system environment is never copied in.** A loader that copies the whole environment into its store and saves it writes `PATH`, `HOME`, and anything else the shell had into the application's file. `Env` holds only what the program sets and what the file contains. `get` reads the process environment as a fallback, and `system` reads it directly, but neither copies anything into the `Env`.

**A prefix instead of bare names in the process environment.** Exporting `PORT` or `DEBUG` as-is collides with variables other software uses. Prefixing only the process-environment copies keeps the file and the code readable while keeping the exported names distinct. The prefix is one value per `Env`, so one call to `with_prefix` moves every exported name.

**No interpolation.** `${VAR}` expansion would let the contents of a file depend on whatever environment the program happens to start in, and would let a value read other variables, including secrets, that the file's author never named. A value is exactly the text in the file. When other tools that do interpolate read the same file, the `\$` escape that `env-lib` writes keeps them from expanding new values. Values written by hand without quotes are read literally by `env-lib` but may be expanded by those tools.

**Errors over silent skipping.** A `.env` loader that skips lines it cannot parse turns a typo into a missing setting that surfaces somewhere else. `env-lib` stops at the first bad line and reports where it is.

**Lossless saves.** The parser keeps every line it reads, including the exact text and inline comment of each entry. A save reads the file again, applies only the variables the program set or removed, and re-renders only those entries. That is what lets a program and a person edit the same file, even while the program is running.

**`export` is `unsafe`.** Writing the process environment while another thread reads it is undefined behavior on Unix-like platforms, and edition 2024 marks `std::env::set_var` `unsafe` on every platform for that reason. `export` inherits that contract rather than hiding it. `get` and `apply_to` cover most needs without it.

**One dependency.** `path-lib`, a first-party crate with no dependencies of its own, provides home expansion, the application config directory, and the atomic write. Everything else is the standard library.

<br>

## Not Included

These are outside the 1.0 scope:

- **Layered files** such as `.env` plus `.env.local`. Open two `Env`s and read from the more specific one first.
- **File watching and automatic reload.** Call `reload` when the file may have changed.
- **Encrypted values.** Keep secrets that need encryption at rest in a secrets manager.
- **`${VAR}` interpolation.** Deliberately not supported; see the [design notes](#design-notes).
- **Per-variable prefixes.** One namespace prefix applies to every variable of an `Env`. Use separate `Env`s for separate namespaces.
