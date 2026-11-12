//! [`Env`]: an application's own environment variables.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
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
///
/// It bounds the input, and with it the memory a parse needs: one owned
/// copy of each run of comment and blank lines, and about 300 bytes plus
/// the text for each entry. A file at the limit made only of very short
/// entries (about 130,000 of them) takes about 40 MiB while it is loaded.
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
/// - **Values** never appear in `Debug` output or in errors. They are
///   held in ordinary `String`s, so they are not wiped from memory when
///   dropped, and cloning an `Env` copies them.
/// - **Files** are checked before they are read: a file other accounts
///   can change, a file owned by another account, and anything that is
///   not a regular file (a FIFO, a device) are refused. See
///   [`open`](Env::open).
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
    /// How the file is checked before it is read.
    check: Check,
    /// The application, when the file is its config `.env`, so a save can
    /// create and check the config directory through it.
    app: Option<path_lib::App>,
    prefix: String,
    values: HashMap<String, String>,
    /// Keys in first-appearance order, for iteration and for appending
    /// new entries on save. After a removal, a removed key stays here
    /// until the next compaction, and only the position `index` names is
    /// live.
    order: Vec<String>,
    /// The live position in `order` of every key held, built on the first
    /// removal (until then every position is live), so removing a key
    /// costs no scan of `order` and loading a file pays nothing for it.
    index: Option<HashMap<String, usize>>,
    /// The file as last loaded or saved, for lossless rewrites.
    doc: Document,
    /// Keys set or removed since the last load or save, in order. A save
    /// applies exactly these to the file as it is on disk at that moment.
    changed: Vec<String>,
    /// The same keys, for constant-time membership tests.
    touched: HashSet<String>,
    /// Keys removed since the last export, so `export` and `apply_to` can
    /// clear them from the target environment.
    removed: BTreeSet<String>,
}

/// What a file must pass before it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Only that it is a regular file ([`Env::open_unchecked`]).
    RegularFile,
    /// [`path_lib::check_owner`] with this level, on the open handle.
    Owner(path_lib::Others),
}

impl Env {
    /// An empty `Env` not tied to any file. Use [`save_to`](Env::save_to)
    /// to write it out.
    #[must_use]
    pub fn new() -> Self {
        Self {
            file: None,
            check: Check::Owner(path_lib::Others::MayRead),
            app: None,
            prefix: DEFAULT_PREFIX.to_owned(),
            values: HashMap::new(),
            order: Vec::new(),
            index: None,
            doc: Document::default(),
            changed: Vec::new(),
            touched: HashSet::new(),
            removed: BTreeSet::new(),
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
    /// The file is checked before it is read, every time it is read
    /// (here, in [`reload`](Env::reload), and in [`save`](Env::save)): it
    /// must be a regular file, owned by the current user or an
    /// administrator account (root; `SYSTEM` or `Administrators`), and no
    /// other account may be able to change it (no group or other write bit
    /// on Unix, no write right for another account in its access control
    /// list on Windows). The check is made on the open file, which is then
    /// the file read, and on Unix the file is opened without blocking, so a
    /// FIFO planted in its place is refused instead of hanging the
    /// program. A symbolic link at `path` is followed only when the
    /// directory holding it passes the same check, so nobody else could
    /// have planted it. Use [`open_unchecked`](Env::open_unchecked) for a
    /// development setup that cannot pass, such as a checkout on a shared
    /// Windows drive.
    ///
    /// # Errors
    ///
    /// - [`Error::Syntax`] with the line number for a malformed line.
    /// - [`Error::TooLarge`] beyond [`MAX_FILE_BYTES`].
    /// - [`Error::Path`] with [`path_lib::Error::Refused`] if the file, or
    ///   the directory holding a link to it, fails the check; the reason
    ///   says what to fix.
    /// - [`Error::Io`] if the file exists but cannot be read, or is not a
    ///   regular file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path.as_ref(), Check::Owner(path_lib::Others::MayRead), None)
    }

    /// Like [`open`](Env::open), but without the owner and permission
    /// check: the file only has to be a regular file. For development
    /// setups only, such as a project `.env` in a checkout that other
    /// accounts can write to; never for a file a service trusts.
    ///
    /// # Errors
    ///
    /// As for [`open`](Env::open), except that the file is not refused for
    /// its owner or permissions.
    pub fn open_unchecked(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path.as_ref(), Check::RegularFile, None)
    }

    fn open_with(path: &Path, check: Check, app: Option<path_lib::App>) -> Result<Self> {
        let path = expand(path)?;
        let mut env = Self::new();
        env.check = check;
        if let Some(text) = read_file(&path, check)? {
            env.load_document(dotenv::parse(&text, Some(&path))?);
        }
        env.file = Some(path);
        env.app = app;
        Ok(env)
    }

    /// Opens the `.env` file in an application's config directory, as
    /// resolved by `path-lib` (for example
    /// `~/.config/HiveDB/.env` on Linux).
    ///
    /// The file is checked as by [`open`](Env::open). For an application
    /// in [`Scope::Global`](path_lib::Scope::Global), a machine-wide
    /// service, the file must also be unreadable by other accounts, since
    /// it holds the service's secrets. A save creates the config directory
    /// with [`App::ensure`](path_lib::App::ensure), which checks it if it
    /// already exists, and writes a new file private to the current user:
    /// mode `0600` on Unix, and on Windows a protected access control list
    /// for the current user, `SYSTEM`, and `Administrators`, even under
    /// `%ProgramData%`.
    ///
    /// # Errors
    ///
    /// As for [`open`](Env::open); also [`Error::Path`] with
    /// [`path_lib::Error::Refused`] when a global application's file is
    /// readable by other accounts.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let app = path_lib::App::new("HiveDB")?;
    /// let env = env_lib::Env::for_app(&app)?;
    /// # Ok::<(), env_lib::Error>(())
    /// ```
    pub fn for_app(app: &path_lib::App) -> Result<Self> {
        let others = if app.scope() == path_lib::Scope::Global {
            path_lib::Others::NoAccess
        } else {
            path_lib::Others::MayRead
        };
        Self::open_with(
            &app.config_at(FILE_NAME)?,
            Check::Owner(others),
            Some(app.clone()),
        )
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
    /// (`JG_`). Programs that are not part of one family of tools should
    /// set their own, so they do not read each other's `JG_` variables.
    ///
    /// End the prefix with `_` (`HIVE_`). Only then is a prefixed name
    /// (`HIVE_PORT`) treated as the same variable as the plain one
    /// (`PORT`). A prefix without the separator (`HIVE`) is still added
    /// on export (`HIVEPORT`), but names are never stripped of it, so
    /// `APPLE` stays `APPLE` under the prefix `APP`.
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
        if let Some(stored) = self.stored_key(key) {
            return self.values.get(stored).map(|v| Cow::Borrowed(v.as_str()));
        }
        let key = self.unprefixed(key);
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
        let key = match self.stored_key(key) {
            Some(stored) => stored.to_owned(),
            None => self.unprefixed(key).to_owned(),
        };
        let value = value.into();
        key::check_key(&key)?;
        key::check_value(&key, &value)?;
        let _was_removed = self.removed.remove(&key);
        match self.values.get_mut(&key) {
            Some(current) if *current == value => {}
            Some(current) => {
                *current = value;
                self.touch(&key);
            }
            None => {
                self.touch(&key);
                if let Some(index) = &mut self.index {
                    let _previous = index.insert(key.clone(), self.order.len());
                }
                self.order.push(key.clone());
                let _previous = self.values.insert(key, value);
                self.compact_if_sparse();
            }
        }
        Ok(())
    }

    /// Removes a variable from this `Env`. Returns whether it was present.
    /// The next save drops it from the file, and the next export removes
    /// it from the target environment.
    pub fn remove(&mut self, key: &str) -> bool {
        let Some(key) = self.stored_key(key).map(str::to_owned) else {
            return false;
        };
        let _value = self.values.remove(&key);
        let _position = self.index_mut().remove(&key);
        self.touch(&key);
        let _new = self.removed.insert(key);
        self.compact_if_sparse();
        true
    }

    /// Removes every variable. The next save leaves only the file's
    /// comments and blank lines, and the next export removes them all from
    /// the target environment.
    pub fn clear(&mut self) {
        self.index = None;
        for key in std::mem::take(&mut self.order) {
            if self.values.remove(&key).is_some() {
                self.touch(&key);
                let _new = self.removed.insert(key);
            }
        }
    }

    /// Whether this `Env` holds the variable (the process environment is
    /// not consulted).
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.stored_key(key).is_some()
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
            .enumerate()
            .filter(|(i, k)| {
                self.index
                    .as_ref()
                    .is_none_or(|index| index.get(*k) == Some(i))
            })
            .filter_map(|(_, k)| self.values.get(k).map(|v| (k.as_str(), v.as_str())))
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
    /// the owner, since `.env` files often hold secrets: mode `0600` on
    /// Unix, and on Windows a protected access control list for the current
    /// user, `SYSTEM`, and `Administrators`, set before anything is written
    /// (`path_lib::write_atomic_private`). Missing directories are created
    /// private too. An existing private file stays private; an existing
    /// file that is not keeps its Unix permissions (on Windows it inherits
    /// the directory's list).
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
    /// written), [`Error::Path`] if the existing file fails the check of
    /// [`open`](Env::open) or the atomic write fails (for example, the
    /// directory cannot be created), or [`Error::Io`] if the file cannot be
    /// read.
    pub fn save_to(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = expand(path.as_ref())?;
        self.compact();
        let disk = match read_file(&path, self.check)? {
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
        let mut dropped: HashSet<&str> = HashSet::new();
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
                        let _new = dropped.insert(key.as_str());
                    }
                }
            }
        }
        if !dropped.is_empty() {
            order.retain(|k| !dropped.contains(k.as_str()));
        }
        let doc = merge(&disk, &values, &order);
        // The application's config directory is created, or checked,
        // through path-lib when this is the application's own file.
        let app_file = self
            .app
            .as_ref()
            .filter(|app| app.config_at(FILE_NAME).is_ok_and(|own| own == path));
        if let Some(app) = app_file {
            let _config = app.ensure(path_lib::Kind::Config)?;
        }
        let keep_app = app_file.is_some();
        write_file(&path, &doc.render())?;
        // Variables that vanished from the file (removed by hand) must also
        // leave the process environment on the next export.
        for key in &self.order {
            if !values.contains_key(key) && !self.removed.contains(key) {
                let _new = self.removed.insert(key.clone());
            }
        }
        self.values = values;
        self.order = order;
        self.rebuild_index();
        self.doc = doc;
        self.file = Some(path);
        if !keep_app {
            self.app = None;
        }
        self.changed.clear();
        self.touched.clear();
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
        let fresh =
            Self::open_with(&path, self.check, self.app.clone())?.with_prefix(&self.prefix)?;
        let mut removed = std::mem::take(&mut self.removed);
        for key in &self.order {
            if !fresh.values.contains_key(key) && !removed.contains(key) {
                let _new = removed.insert(key.clone());
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
            let _cmd = cmd.env(self.export_name(key).as_ref(), value);
        }
        for key in &self.removed {
            let _cmd = cmd.env_remove(self.export_name(key).as_ref());
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
            unsafe { std::env::set_var(self.export_name(key).as_ref(), value) };
        }
        for key in std::mem::take(&mut self.removed) {
            // SAFETY: as above.
            unsafe { std::env::remove_var(self.export_name(&key).as_ref()) };
        }
    }

    // -----------------------------------------------------------------

    fn prefixed(&self, key: &str) -> String {
        let mut name = String::with_capacity(self.prefix.len() + key.len());
        name.push_str(&self.prefix);
        name.push_str(key);
        name
    }

    /// Whether prefixed names are recognized: only when the prefix ends
    /// in the `_` separator, so a prefix such as `APP` never eats the start
    /// of `APPLE`.
    fn strips(&self) -> bool {
        self.prefix.ends_with('_')
    }

    /// Strips the prefix from an already-prefixed name, so `JG_PORT` and
    /// `PORT` address the same variable.
    fn unprefixed<'k>(&self, key: &'k str) -> &'k str {
        if !self.strips() {
            return key;
        }
        match key.strip_prefix(self.prefix.as_str()) {
            Some(rest) if !rest.is_empty() => rest,
            _ => key,
        }
    }

    /// The name under which this `Env` stores `key`, if it holds it:
    /// `key` itself, or its plain form (`PORT` for `JG_PORT`), or its
    /// prefixed form (`JG_PORT` written in the file, for `PORT`).
    fn stored_key(&self, key: &str) -> Option<&str> {
        if let Some((stored, _)) = self.values.get_key_value(key) {
            return Some(stored);
        }
        if !self.strips() {
            return None;
        }
        let plain = self.unprefixed(key);
        if plain.len() != key.len() {
            return self.values.get_key_value(plain).map(|(k, _)| k.as_str());
        }
        let full = self.prefixed(key);
        self.values
            .get_key_value(full.as_str())
            .map(|(k, _)| k.as_str())
    }

    /// The name a stored key is exported under: prefixed, unless the file
    /// already spells it with the prefix.
    fn export_name<'k>(&self, key: &'k str) -> Cow<'k, str> {
        if self.strips() && self.unprefixed(key).len() != key.len() {
            Cow::Borrowed(key)
        } else {
            Cow::Owned(self.prefixed(key))
        }
    }

    fn load_document(&mut self, doc: Document) {
        let (values, order) = values_of(&doc);
        self.values = values;
        self.order = order;
        self.rebuild_index();
        self.doc = doc;
    }

    /// `order` was replaced by a clean list: every position is live.
    fn rebuild_index(&mut self) {
        self.index = None;
    }

    /// The position index, built from `order` the first time it is needed.
    fn index_mut(&mut self) -> &mut HashMap<String, usize> {
        let order = &self.order;
        self.index.get_or_insert_with(|| {
            order
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect()
        })
    }

    /// Drops the removed keys from `order`.
    fn compact(&mut self) {
        let Some(index) = self.index.take() else {
            return;
        };
        let live: Vec<String> = std::mem::take(&mut self.order)
            .into_iter()
            .enumerate()
            .filter(|(i, k)| index.get(k) == Some(i))
            .map(|(_, k)| k)
            .collect();
        self.order = live;
    }

    /// Compacts once removed keys outnumber live ones, so bulk removals
    /// cost amortized constant time per key.
    fn compact_if_sparse(&mut self) {
        let live = self.index.as_ref().map_or(self.order.len(), HashMap::len);
        if self.order.len() > 2 * live + 32 {
            self.compact();
        }
    }

    fn touch(&mut self, key: &str) {
        if self.touched.insert(key.to_owned()) {
            self.changed.push(key.to_owned());
        }
    }

    /// This `Env` merged into the layout it was loaded from.
    #[cfg(test)]
    fn merged(&self) -> Document {
        let order: Vec<String> = self.iter().map(|(k, _)| k.to_owned()).collect();
        merge(&self.doc, &self.values, &order)
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
            .field("keys", &self.iter().map(|(k, _)| k).collect::<Vec<_>>())
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
        if let Line::Entry(entry) = line {
            if values
                .insert(entry.key.clone(), entry.value.clone())
                .is_none()
            {
                order.push(entry.key.clone());
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
        if let Line::Entry(entry) = line {
            let _previous = last.insert(entry.key.as_str(), i);
        }
    }
    let mut lines = Vec::with_capacity(base.lines.len() + values.len());
    for (i, line) in base.lines.iter().enumerate() {
        match line {
            Line::Other(_) => lines.push(line.clone()),
            Line::Entry(old) => {
                let Some(current) = values.get(&old.key) else {
                    continue;
                };
                if last.get(old.key.as_str()) != Some(&i) {
                    continue;
                }
                if *current == old.value {
                    lines.push(line.clone());
                } else {
                    lines.push(entry(&old.key, current, old.export, &old.comment));
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
    Line::Entry(dotenv::Entry {
        key: key.to_owned(),
        value: value.to_owned(),
        raw,
        export,
        comment: comment.to_owned(),
    })
}

/// Expands a leading `~` when the path is valid Unicode.
fn expand(path: &Path) -> Result<PathBuf> {
    match path.to_str() {
        Some(s) if s.starts_with('~') => Ok(path_lib::expand_home(s)?),
        _ => Ok(path.to_path_buf()),
    }
}

/// Reads a file as UTF-8 after `check`, or `None` if it does not exist.
fn read_file(path: &Path, check: Check) -> Result<Option<String>> {
    let Some(real) = resolve(path, check)? else {
        return Ok(None);
    };
    let meta = match fs::metadata(&real) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io("inspect", path, e)),
    };
    if !meta.is_file() {
        return Err(Error::io(
            "open",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file (a directory, FIFO, socket, or device)",
            ),
        ));
    }
    let file = match check {
        Check::RegularFile => match fs::File::open(&real) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io("open", path, e)),
        },
        Check::Owner(others) => match path_lib::open_checked(&real, others) {
            Ok(file) => file,
            Err(path_lib::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(Error::Path(e)),
        },
    };
    let opened = file.metadata().map_err(|e| Error::io("inspect", path, e))?;
    if !opened.is_file() {
        return Err(Error::io(
            "open",
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }
    let len = opened.len();
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

/// The file `path` names: `path` itself, or the target of a symbolic link
/// at `path`. A link is followed only when the directory holding it passes
/// the owner check, so no other account could have planted it. `None`
/// when nothing is there (a broken link counts as nothing).
fn resolve(path: &Path, check: Check) -> Result<Option<PathBuf>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io("inspect", path, e)),
    };
    if !meta.file_type().is_symlink() {
        return Ok(Some(path.to_path_buf()));
    }
    if check != Check::RegularFile {
        let parent = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(parent) => parent.to_path_buf(),
            None => std::env::current_dir().map_err(|e| Error::io("inspect", path, e))?,
        };
        path_lib::check_owner(&parent, path_lib::Others::MayRead)?;
    }
    match fs::canonicalize(path) {
        Ok(real) => Ok(Some(real)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("open", path, e)),
    }
}

/// Writes the rendered file atomically. A new file, or one that is already
/// private, is written private (`0600` on Unix, a protected access control
/// list on Windows), with missing directories created private too; an
/// existing file that is not private keeps its Unix permissions.
fn write_file(path: &Path, text: &str) -> Result<()> {
    let private = match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Ok(meta) if meta.is_file() => {
            let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            path_lib::check_owner_private(real).is_ok()
        }
        // A directory or other entry: let the write report it.
        _ => false,
    };
    if private {
        path_lib::write_atomic_private(path, text)?;
    } else {
        path_lib::write_atomic(path, text)?;
    }
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
