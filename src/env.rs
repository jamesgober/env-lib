//! [`Env`]: an application's own environment variables.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::dotenv::{self, Document, Line};
use crate::error::{Error, Result};
use crate::key::{self, DEFAULT_PREFIX};

/// Largest `.env` file that will be read, in bytes.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The name [`Env::for_app`] gives the file in an application's config
/// directory.
pub const FILE_NAME: &str = ".env";

/// An application's own environment variables, optionally backed by a
/// `.env` file.
///
/// `Env` holds only the variables your program sets and the ones in its
/// file; the system environment is never copied into it and never saved.
/// Reads fall back to the process environment under the [prefix], so
/// variables that were exported (or set by a parent process) are visible
/// too.
///
/// - **Names** are plain (`PORT`) in code and in the file. The prefix is
///   added only when exporting to the process environment (`JG_PORT` by
///   default) so application variables never collide with system ones.
///   Passing an already-prefixed name (`JG_PORT`) is accepted and means
///   the same variable.
/// - **Saving** re-reads the file and applies this session's changes to
///   it: variables set here are updated in place (keeping any inline
///   comment) or appended, variables removed here are dropped, and
///   everything else in the file, including edits made by hand while the
///   program ran, comments, blank lines, and ordering, is kept as written.
///   When the file and the program changed the same variable, the
///   program's value wins. The write is atomic.
/// - **Values** never appear in `Debug` output or in errors.
///
/// [prefix]: Env::prefix
///
/// # Examples
///
/// ```
/// use env_lib::Env;
///
/// let dir = std::env::temp_dir().join("env-lib-doc-env");
/// let file = dir.join(".env");
///
/// let mut env = Env::open(&file)?;      // missing file is fine
/// env.set("PORT", "7000")?;
/// env.set("DB_URL", "postgres://localhost/app")?;
/// env.save()?;
///
/// let env = Env::open(&file)?;          // next run
/// assert_eq!(env.get("PORT").as_deref(), Some("7000"));
/// assert_eq!(env.parse::<u16>("PORT")?, Some(7000));
/// # std::fs::remove_dir_all(&dir).ok();
/// # Ok::<(), env_lib::Error>(())
/// ```
#[derive(Clone)]
pub struct Env {
    file: Option<PathBuf>,
    prefix: String,
    values: HashMap<String, String>,
    /// Keys in first-appearance order, for iteration and for appending
    /// new entries on save.
    order: Vec<String>,
    /// The file as last loaded or saved, for lossless rewrites.
    doc: Document,
    /// Keys set or removed since the last load or save, in order. A save
    /// applies exactly these to the file as it is on disk at that moment.
    changed: Vec<String>,
    /// Keys removed since the last export, so `export` and `apply_to` can
    /// clear them from the target environment.
    removed: Vec<String>,
}

impl Env {
    /// An empty `Env` not tied to any file. Use [`save_to`](Env::save_to)
    /// to write it out.
    #[must_use]
    pub fn new() -> Self {
        Self {
            file: None,
            prefix: DEFAULT_PREFIX.to_owned(),
            values: HashMap::new(),
            order: Vec::new(),
            doc: Document::default(),
            changed: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Loads the `.env` file at `path`, or starts empty if it does not
    /// exist yet. Either way the `Env` is tied to `path` for
    /// [`save`](Env::save) and [`reload`](Env::reload). A leading `~` is
    /// expanded to the home directory.
    ///
    /// When a variable appears more than once in the file, the last value
    /// wins (and a save keeps only that line).
    ///
    /// # Errors
    ///
    /// - [`Error::Syntax`] with the line number for a malformed line.
    /// - [`Error::TooLarge`] beyond [`MAX_FILE_BYTES`].
    /// - [`Error::Io`] if the file exists but cannot be read.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand(path.as_ref())?;
        let mut env = Self::new();
        if let Some(text) = read_file(&path)? {
            env.load_document(dotenv::parse(&text, Some(&path))?);
        }
        env.file = Some(path);
        Ok(env)
    }

    /// Opens the `.env` file in an application's config directory, as
    /// resolved by `path-lib` (for example
    /// `~/.config/HiveDB/.env` on Linux).
    ///
    /// # Errors
    ///
    /// As for [`open`](Env::open).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let app = path_lib::App::new("HiveDB")?;
    /// let env = env_lib::Env::for_app(&app)?;
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn for_app(app: &path_lib::App) -> Result<Self> {
        Self::open(app.config_at(FILE_NAME)?)
    }

    /// Parses `.env` text without tying the result to a file.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] for a malformed line.
    ///
    /// # Examples
    ///
    /// ```
    /// let env = env_lib::Env::from_text("A=1\nB=\"two words\"\n")?;
    /// assert_eq!(env.get("B").as_deref(), Some("two words"));
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn from_text(text: &str) -> Result<Self> {
        let mut env = Self::new();
        env.load_document(dotenv::parse(text, None)?);
        Ok(env)
    }

    /// Sets the prefix used when exporting to, and reading from, the
    /// process environment. An empty prefix means [`DEFAULT_PREFIX`]
    /// (`JG_`).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPrefix`] unless the prefix is empty or a valid name
    /// (ASCII letters, digits, `_`, not starting with a digit).
    ///
    /// # Examples
    ///
    /// ```
    /// let env = env_lib::Env::new().with_prefix("HIVE_")?;
    /// assert_eq!(env.prefix(), "HIVE_");
    /// assert_eq!(env_lib::Env::new().with_prefix("")?.prefix(), "JG_");
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn with_prefix(mut self, prefix: impl AsRef<str>) -> Result<Self> {
        self.prefix = key::normalize_prefix(prefix.as_ref())?;
        Ok(self)
    }

    /// The prefix for the process environment, `JG_` unless changed.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The file this `Env` saves to and reloads from, if any.
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// A variable's value: this `Env`'s own value if it has one, otherwise
    /// the process environment's `<prefix><key>` (for example `JG_PORT`).
    ///
    /// Returns `None` if neither has it, if the process value is not valid
    /// Unicode, or if `key` is not a valid name.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Cow<'_, str>> {
        let key = self.unprefixed(key);
        if let Some(value) = self.values.get(key) {
            return Some(Cow::Borrowed(value));
        }
        if key::check_name(key).is_err() {
            return None;
        }
        std::env::var(self.prefixed(key)).ok().map(Cow::Owned)
    }

    /// [`get`](Env::get), or `default` when the variable is not set.
    ///
    /// # Examples
    ///
    /// ```
    /// let env = env_lib::Env::new();
    /// assert_eq!(env.get_or("LOG_LEVEL", "info"), "info");
    /// ```
    #[must_use]
    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> Cow<'a, str> {
        self.get(key).unwrap_or(Cow::Borrowed(default))
    }

    /// Parses a variable into any type that implements [`FromStr`].
    /// `Ok(None)` when it is not set.
    ///
    /// # Errors
    ///
    /// [`Error::Parse`] when it is set but does not parse. The value is
    /// not included in the error.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut env = env_lib::Env::new();
    /// env.set("WORKERS", "8")?;
    /// assert_eq!(env.parse::<usize>("WORKERS")?, Some(8));
    /// assert_eq!(env.parse::<usize>("MISSING")?, None);
    /// env.set("WORKERS", "eight")?;
    /// assert!(env.parse::<usize>("WORKERS").is_err());
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn parse<T: FromStr>(&self, key: &str) -> Result<Option<T>> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        value
            .trim()
            .parse::<T>()
            .map(Some)
            .map_err(|_| Error::Parse {
                key: self.unprefixed(key).chars().take(64).collect(),
                type_name: std::any::type_name::<T>(),
            })
    }

    /// Reads a variable as an on/off switch.
    ///
    /// Accepts, ignoring case and surrounding spaces, `true`, `1`, `yes`,
    /// `on` as `true` and `false`, `0`, `no`, `off`, and the empty string as
    /// `false`. `Ok(None)` when it is not set.
    ///
    /// # Errors
    ///
    /// [`Error::Parse`] for any other value. The value is not included in
    /// the error.
    ///
    /// # Examples
    ///
    /// ```
    /// let env = env_lib::Env::from_text("DEBUG=yes\nCACHE=Off\n")?;
    /// assert_eq!(env.flag("DEBUG")?, Some(true));
    /// assert_eq!(env.flag("CACHE")?, Some(false));
    /// assert_eq!(env.flag("MISSING")?, None);
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn flag(&self, key: &str) -> Result<Option<bool>> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        let value = value.trim();
        let truthy = ["true", "1", "yes", "on"];
        let falsy = ["false", "0", "no", "off", ""];
        if truthy.iter().any(|t| value.eq_ignore_ascii_case(t)) {
            Ok(Some(true))
        } else if falsy.iter().any(|f| value.eq_ignore_ascii_case(f)) {
            Ok(Some(false))
        } else {
            Err(Error::Parse {
                key: self.unprefixed(key).chars().take(64).collect(),
                type_name: "flag (true/false, 1/0, yes/no, on/off)",
            })
        }
    }

    /// Sets a variable in this `Env`. It is saved by the next
    /// [`save`](Env::save) and exported by the next export.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidKey`] or [`Error::InvalidValue`] (NUL, or more than
    /// [`MAX_VALUE_BYTES`](crate::MAX_VALUE_BYTES)).
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        let key = self.unprefixed(key).to_owned();
        let value = value.into();
        key::check_key(&key)?;
        key::check_value(&key, &value)?;
        self.removed.retain(|k| *k != key);
        match self.values.get_mut(&key) {
            Some(current) if *current == value => {}
            Some(current) => {
                *current = value;
                self.touch(&key);
            }
            None => {
                self.touch(&key);
                self.order.push(key.clone());
                let _previous = self.values.insert(key, value);
            }
        }
        Ok(())
    }

    /// Removes a variable from this `Env`. Returns whether it was present.
    /// The next save drops it from the file, and the next export removes
    /// it from the target environment.
    pub fn remove(&mut self, key: &str) -> bool {
        let key = self.unprefixed(key);
        if self.values.remove(key).is_none() {
            return false;
        }
        self.order.retain(|k| k != key);
        self.removed.push(key.to_owned());
        self.touch(key);
        true
    }

    /// Removes every variable. The next save leaves only the file's
    /// comments and blank lines, and the next export removes them all from
    /// the target environment.
    pub fn clear(&mut self) {
        for key in std::mem::take(&mut self.order) {
            let _value = self.values.remove(&key);
            self.touch(&key);
            self.removed.push(key);
        }
    }

    /// Whether this `Env` holds the variable (the process environment is
    /// not consulted).
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(self.unprefixed(key))
    }

    /// Number of variables this `Env` holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether this `Env` holds no variables.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The variables this `Env` holds, in the order they first appeared
    /// (file order, then insertion order).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.order
            .iter()
            .filter_map(|k| self.values.get(k).map(|v| (k.as_str(), v.as_str())))
    }

    /// Whether there are changes not yet saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.changed.is_empty()
    }

    /// Applies this session's changes to this `Env`'s file and writes it
    /// (see the type documentation for exactly what is kept).
    ///
    /// The file is read again first, so edits made to it by hand while
    /// the program was running are kept, and after the save this `Env`
    /// holds them too.
    ///
    /// The write is atomic (`path_lib::write_atomic`): readers see the old
    /// file or the new one, never a mix. A newly created file is private to
    /// the owner on Unix (`0600`), since `.env` files often hold secrets;
    /// an existing file keeps its permissions.
    ///
    /// # Errors
    ///
    /// [`Error::NoFile`] if this `Env` is not tied to a file; otherwise as
    /// for [`save_to`](Env::save_to).
    pub fn save(&mut self) -> Result<()> {
        let path = self.file.clone().ok_or(Error::NoFile)?;
        self.save_to(path)
    }

    /// Writes the variables to `path` and ties this `Env` to it.
    ///
    /// When `path` is this `Env`'s own file, only this session's changes
    /// are applied, as for [`save`](Env::save). For any other file, every
    /// variable this `Env` holds is written into it (and variables removed
    /// this session are removed from it); the file's other variables,
    /// comments, and layout are kept.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] if the existing file is malformed (nothing is
    /// written), [`Error::Path`] if the atomic write fails (for example,
    /// the directory cannot be created), or [`Error::Io`] if the file cannot
    /// be read or a new file's permissions cannot be set.
    pub fn save_to(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = expand(path.as_ref())?;
        let disk = match read_file(&path)? {
            Some(text) => dotenv::parse(&text, Some(&path))?,
            None => Document::default(),
        };
        let (mut values, mut order) = values_of(&disk);
        let own_file = self.file.as_deref() == Some(path.as_path());
        let keys: Vec<&String> = if own_file {
            self.changed.iter().collect()
        } else {
            self.order.iter().chain(&self.changed).collect()
        };
        for key in keys {
            match self.values.get(key) {
                Some(value) => {
                    if !values.contains_key(key) {
                        order.push(key.clone());
                    }
                    let _previous = values.insert(key.clone(), value.clone());
                }
                None => {
                    if values.remove(key).is_some() {
                        order.retain(|k| k != key);
                    }
                }
            }
        }
        let doc = merge(&disk, &values, &order);
        create_private(&path)?;
        path_lib::write_atomic(&path, doc.render())?;
        // Variables that vanished from the file (removed by hand) must also
        // leave the process environment on the next export.
        for key in &self.order {
            if !values.contains_key(key) && !self.removed.contains(key) {
                self.removed.push(key.clone());
            }
        }
        self.values = values;
        self.order = order;
        self.doc = doc;
        self.file = Some(path);
        self.changed.clear();
        Ok(())
    }

    /// Discards unsaved changes and loads the file again, picking up any
    /// edits made outside the program.
    ///
    /// # Errors
    ///
    /// [`Error::NoFile`] if this `Env` is not tied to a file; otherwise as
    /// for [`open`](Env::open).
    pub fn reload(&mut self) -> Result<()> {
        let path = self.file.clone().ok_or(Error::NoFile)?;
        let fresh = Self::open(&path)?.with_prefix(&self.prefix)?;
        let mut removed = std::mem::take(&mut self.removed);
        for key in &self.order {
            if !fresh.values.contains_key(key) && !removed.contains(key) {
                removed.push(key.clone());
            }
        }
        *self = fresh;
        self.removed = removed;
        Ok(())
    }

    /// Adds this `Env`'s variables, prefixed, to a child process's
    /// environment, and removes variables deleted since the last export.
    /// The parent's own environment is not touched, so this is always
    /// safe.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut env = env_lib::Env::new();
    /// env.set("PORT", "7000")?;
    /// let mut cmd = std::process::Command::new("worker");
    /// env.apply_to(&mut cmd);
    /// let vars: Vec<_> = cmd.get_envs().collect();
    /// assert_eq!(vars[0].0, "JG_PORT");
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn apply_to<'c>(&self, cmd: &'c mut Command) -> &'c mut Command {
        for (key, value) in self.iter() {
            let _cmd = cmd.env(self.prefixed(key), value);
        }
        for key in &self.removed {
            let _cmd = cmd.env_remove(self.prefixed(key));
        }
        cmd
    }

    /// Exports this `Env`'s variables, prefixed, into the current process
    /// environment, and removes variables deleted since the last export,
    /// so code that reads `std::env::var("JG_PORT")` directly sees them.
    ///
    /// Prefer [`get`](Env::get) and [`apply_to`](Env::apply_to), which
    /// need no `unsafe`; use this only for third-party code that reads the
    /// process environment itself.
    ///
    /// # Safety
    ///
    /// Changing the process environment is unsound if any other thread
    /// reads or writes it at the same time, including indirectly through C
    /// functions such as `getenv` (DNS resolution and time-zone handling
    /// do this). Call it during startup, before spawning threads or
    /// starting an async runtime, or while you can otherwise guarantee no
    /// concurrent environment access. This is the same contract as
    /// [`std::env::set_var`].
    #[allow(unsafe_code)]
    pub unsafe fn export(&mut self) {
        for (key, value) in self.iter() {
            // SAFETY: the caller guarantees no concurrent environment
            // access (this function's safety contract); names are
            // validated and values contain no NUL, so `set_var` cannot
            // panic.
            unsafe { std::env::set_var(self.prefixed(key), value) };
        }
        for key in std::mem::take(&mut self.removed) {
            // SAFETY: as above.
            unsafe { std::env::remove_var(self.prefixed(&key)) };
        }
    }

    // -----------------------------------------------------------------

    fn prefixed(&self, key: &str) -> String {
        let mut name = String::with_capacity(self.prefix.len() + key.len());
        name.push_str(&self.prefix);
        name.push_str(key);
        name
    }

    /// Strips the prefix from an already-prefixed name, so `JG_PORT` and
    /// `PORT` address the same variable.
    fn unprefixed<'k>(&self, key: &'k str) -> &'k str {
        match key.strip_prefix(self.prefix.as_str()) {
            Some(rest) if !rest.is_empty() => rest,
            _ => key,
        }
    }

    fn load_document(&mut self, doc: Document) {
        let (values, order) = values_of(&doc);
        self.values = values;
        self.order = order;
        self.doc = doc;
    }

    fn touch(&mut self, key: &str) {
        if !self.changed.iter().any(|k| k == key) {
            self.changed.push(key.to_owned());
        }
    }

    /// This `Env` merged into the layout it was loaded from.
    #[cfg(test)]
    fn merged(&self) -> Document {
        merge(&self.doc, &self.values, &self.order)
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Env {
    /// Lists file, prefix, and names. Values are never shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Env")
            .field("file", &self.file)
            .field("prefix", &self.prefix)
            .field("keys", &self.order)
            .field("dirty", &self.is_dirty())
            .finish_non_exhaustive()
    }
}

/// Reads the value of a system environment variable exactly as named,
/// without any prefix. `None` when it is unset or not valid Unicode.
///
/// # Examples
///
/// ```
/// # let name = if cfg!(windows) { "SystemRoot" } else { "PATH" };
/// assert!(env_lib::system(name).is_some());
/// ```
#[must_use]
pub fn system(name: &str) -> Option<String> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return None;
    }
    std::env::var(name).ok()
}

/// The variables of a document (last occurrence wins) and their names in
/// first-appearance order.
fn values_of(doc: &Document) -> (HashMap<String, String>, Vec<String>) {
    let mut values = HashMap::new();
    let mut order = Vec::new();
    for line in &doc.lines {
        if let Line::Entry { key, value, .. } = line {
            if values.insert(key.clone(), value.clone()).is_none() {
                order.push(key.clone());
            }
        }
    }
    (values, order)
}

/// `base`'s layout with `values` applied: unchanged entries keep their
/// exact text, changed ones are re-rendered with their inline comment,
/// entries for missing variables and earlier duplicates are dropped, and
/// variables `base` lacks are appended in `order`.
fn merge(base: &Document, values: &HashMap<String, String>, order: &[String]) -> Document {
    let mut last: HashMap<&str, usize> = HashMap::new();
    for (i, line) in base.lines.iter().enumerate() {
        if let Line::Entry { key, .. } = line {
            let _previous = last.insert(key.as_str(), i);
        }
    }
    let mut lines = Vec::with_capacity(base.lines.len() + values.len());
    for (i, line) in base.lines.iter().enumerate() {
        match line {
            Line::Other(_) => lines.push(line.clone()),
            Line::Entry {
                key,
                value,
                export,
                comment,
                ..
            } => {
                let Some(current) = values.get(key) else {
                    continue;
                };
                if last.get(key.as_str()) != Some(&i) {
                    continue;
                }
                if current == value {
                    lines.push(line.clone());
                } else {
                    lines.push(entry(key, current, *export, comment));
                }
            }
        }
    }
    for key in order {
        if !last.contains_key(key.as_str()) {
            if let Some(value) = values.get(key) {
                lines.push(entry(key, value, false, ""));
            }
        }
    }
    Document {
        lines,
        crlf: base.crlf,
    }
}

fn entry(key: &str, value: &str, export: bool, comment: &str) -> Line {
    let mut raw = dotenv::render_entry(key, value, export);
    raw.push_str(comment);
    Line::Entry {
        key: key.to_owned(),
        value: value.to_owned(),
        raw,
        export,
        comment: comment.to_owned(),
    }
}

/// Expands a leading `~` when the path is valid Unicode.
fn expand(path: &Path) -> Result<PathBuf> {
    match path.to_str() {
        Some(s) if s.starts_with('~') => Ok(path_lib::expand_home(s)?),
        _ => Ok(path.to_path_buf()),
    }
}

/// Reads a file as UTF-8, or `None` if it does not exist.
fn read_file(path: &Path) -> Result<Option<String>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io("open", path, e)),
    };
    let len = file
        .metadata()
        .map_err(|e| Error::io("inspect", path, e))?
        .len();
    if len > MAX_FILE_BYTES {
        return Err(Error::TooLarge {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    let _read = file
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io("read", path, e))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(Error::TooLarge {
            path: path.to_path_buf(),
        });
    }
    String::from_utf8(bytes).map(Some).map_err(|e| {
        let valid = &e.as_bytes()[..e.utf8_error().valid_up_to()];
        Error::Syntax {
            path: Some(path.to_path_buf()),
            // Lines before the bad byte, plus the line it is on.
            line: valid.split(|b| *b == b'\n').count(),
            reason: "not valid UTF-8",
        }
    })
}

/// Creates `path` as an empty owner-only file if it does not exist, so the
/// atomic write that follows preserves those permissions.
#[cfg(unix)]
fn create_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        path_lib::make_dir(parent)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(Error::io("create", path, e)),
    }
}

/// Windows: files inherit the directory's access control list; the
/// per-user config directory is already private to the user.
#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)] // Same signature as the Unix version.
fn create_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_set_get_remove_and_dirty_tracking() {
        let mut env = Env::new();
        assert!(!env.is_dirty());
        env.set("A", "1").unwrap();
        assert!(env.is_dirty());
        assert_eq!(env.get("A").as_deref(), Some("1"));
        assert!(env.contains("A"));
        assert_eq!(env.len(), 1);
        assert!(env.remove("A"));
        assert!(!env.remove("A"));
        assert!(env.is_empty());
    }

    #[test]
    fn test_setting_same_value_is_not_a_change() {
        let mut env = Env::from_text("A=1\n").unwrap();
        env.set("A", "1").unwrap();
        assert!(!env.is_dirty());
    }

    #[test]
    fn test_prefixed_and_plain_names_are_the_same_variable() {
        let mut env = Env::new();
        env.set("JG_PORT", "1").unwrap();
        assert_eq!(env.get("PORT").as_deref(), Some("1"));
        assert!(env.contains("JG_PORT"));
        assert_eq!(env.iter().collect::<Vec<_>>(), [("PORT", "1")]);
        // The bare prefix is a name of its own.
        env.set("JG_", "x").unwrap();
        assert_eq!(env.get("JG_").as_deref(), Some("x"));
    }

    #[test]
    fn test_set_rejects_bad_names_and_values() {
        let mut env = Env::new();
        assert!(matches!(env.set("1X", "v"), Err(Error::InvalidKey { .. })));
        assert!(matches!(env.set("A.B", "v"), Err(Error::InvalidKey { .. })));
        assert!(matches!(
            env.set("A", "v\0"),
            Err(Error::InvalidValue { .. })
        ));
        assert!(env.is_empty());
    }

    #[test]
    fn test_get_misses_and_invalid_names_never_reach_the_os() {
        let env = Env::new();
        assert_eq!(env.get("ENV_LIB_TEST_NOT_SET_ANYWHERE"), None);
        assert_eq!(env.get("bad-name"), None);
        let name = if cfg!(windows) { "SystemRoot" } else { "PATH" };
        assert!(system(name).is_some());
        assert_eq!(system(""), None);
        assert_eq!(system("A=B"), None);
        assert_eq!(system("A\0B"), None);
    }

    #[test]
    fn test_get_or_and_parse() {
        let mut env = Env::new();
        assert_eq!(env.get_or("X", "d"), "d");
        env.set("X", " 42 ").unwrap();
        assert_eq!(env.get_or("X", "d"), " 42 ");
        assert_eq!(env.parse::<u32>("X").unwrap(), Some(42));
        assert_eq!(env.parse::<u32>("Y").unwrap(), None);
        env.set("X", "x").unwrap();
        let err = env.parse::<u32>("X").unwrap_err();
        assert!(matches!(
            err,
            Error::Parse {
                type_name: "u32",
                ..
            }
        ));
        env.set("SECRET", "hunter2").unwrap();
        let err = env.parse::<u32>("SECRET").unwrap_err().to_string();
        assert!(!err.contains("hunter2"), "{err}");
    }

    #[test]
    fn test_flag_accepts_common_switch_spellings() {
        let env = Env::from_text(
            "T1=true\nT2=1\nT3= YES \nT4=On\nF1=false\nF2=0\nF3=no\nF4=OFF\nF5=\nBAD=maybe\n",
        )
        .unwrap();
        for k in ["T1", "T2", "T3", "T4"] {
            assert_eq!(env.flag(k).unwrap(), Some(true), "{k}");
        }
        for k in ["F1", "F2", "F3", "F4", "F5"] {
            assert_eq!(env.flag(k).unwrap(), Some(false), "{k}");
        }
        assert_eq!(env.flag("NONE").unwrap(), None);
        let err = env.flag("BAD").unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
        assert!(!err.to_string().contains("maybe"));
    }

    #[test]
    fn test_clear_removes_all_and_marks_for_export_removal() {
        let mut env = Env::from_text("# c\nA=1\nB=2\n").unwrap();
        env.clear();
        assert!(env.is_empty());
        assert!(env.is_dirty());
        assert_eq!(env.merged().render(), "# c\n");
        let mut cmd = Command::new("x");
        let _cmd = env.apply_to(&mut cmd);
        assert_eq!(cmd.get_envs().filter(|(_, v)| v.is_none()).count(), 2);
        let mut empty = Env::new();
        empty.clear();
        assert!(!empty.is_dirty());
    }

    #[test]
    fn test_iteration_follows_file_then_insertion_order() {
        let mut env = Env::from_text("B=1\nA=2\nB=3\n").unwrap();
        env.set("C", "4").unwrap();
        assert_eq!(
            env.iter().collect::<Vec<_>>(),
            [("B", "3"), ("A", "2"), ("C", "4")]
        );
    }

    #[test]
    fn test_merge_preserves_layout_and_applies_changes() {
        let text = "# config\nA=1  # keep\n\nexport B=2  # port\nA=old-dup\nC='lit'\n";
        let mut env = Env::from_text(text).unwrap();
        env.set("B", "two words").unwrap();
        assert!(env.remove("C"));
        env.set("D", "new").unwrap();
        let out = env.merged().render();
        assert_eq!(
            out,
            "# config\n\nexport B=\"two words\"  # port\nA=old-dup\nD=new\n"
        );
    }

    #[test]
    fn test_merge_keeps_unchanged_raw_text_exactly() {
        let text = "A = \"x\"   # spaced\nB='y'\n";
        let env = Env::from_text(text).unwrap();
        assert_eq!(env.merged().render(), text);
    }

    #[test]
    fn test_apply_to_sets_prefixed_and_removes_deleted() {
        let mut env = Env::from_text("A=1\nB=2\n")
            .unwrap()
            .with_prefix("APP_")
            .unwrap();
        assert!(env.remove("B"));
        let mut cmd = Command::new("x");
        let _cmd = env.apply_to(&mut cmd);
        let vars: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(vars.contains(&("APP_A".to_owned(), Some("1".to_owned()))));
        assert!(vars.contains(&("APP_B".to_owned(), None)));
    }

    #[test]
    fn test_debug_never_shows_values() {
        let mut env = Env::new();
        env.set("TOKEN", "s3cr3t-value").unwrap();
        let shown = format!("{env:?}");
        assert!(shown.contains("TOKEN"));
        assert!(!shown.contains("s3cr3t"));
    }

    #[test]
    fn test_save_without_file_is_an_error() {
        let mut env = Env::new();
        assert!(matches!(env.save(), Err(Error::NoFile)));
        assert!(matches!(env.reload(), Err(Error::NoFile)));
    }
}
