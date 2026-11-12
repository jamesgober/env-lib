//! Files are checked before they are trusted, saves are private, and
//! names with or without the prefix are the same variable.

#![allow(clippy::unwrap_used, missing_docs)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use env_lib::{Env, Error};

/// A scratch directory private to the current user, removed on drop.
struct Sandbox {
    root: PathBuf,
    dir: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join("env-lib-checks").join(format!(
            "{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let app = path_lib::App::portable("sandbox", &root).unwrap();
        let dir = app
            .ensure_private(path_lib::Kind::Data)
            .unwrap()
            .to_path_buf();
        Self { root, dir }
    }

    fn file(&self) -> PathBuf {
        self.dir.join(".env")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn refused(r: env_lib::Result<Env>) -> &'static str {
    match r {
        Err(Error::Path(path_lib::Error::Refused { reason, .. })) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// --- prefixes ----------------------------------------------------------------

#[test]
fn test_prefix_without_separator_never_renames_keys() {
    let mut env = Env::new().with_prefix("APP").unwrap();
    env.set("APPLE", "red").unwrap();
    assert_eq!(env.iter().collect::<Vec<_>>(), [("APPLE", "red")]);
    assert_eq!(env.get("APPLE").as_deref(), Some("red"));
    let mut cmd = std::process::Command::new("x");
    let _cmd = env.apply_to(&mut cmd);
    let names: Vec<_> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
    assert_eq!(names, ["APPAPPLE"]);
}

#[test]
fn test_prefix_with_separator_still_strips() {
    let mut env = Env::new().with_prefix("APP_").unwrap();
    env.set("APP_PORT", "1").unwrap();
    assert_eq!(env.iter().collect::<Vec<_>>(), [("PORT", "1")]);
    env.set("APPLE", "red").unwrap();
    assert!(env.contains("APPLE"));
}

#[test]
fn test_prefixed_name_in_the_file_is_reachable_both_ways() {
    let mut env = Env::from_text("JG_TOKEN=abc\n").unwrap();
    assert!(env.contains("JG_TOKEN"));
    assert!(env.contains("TOKEN"));
    assert_eq!(env.get("JG_TOKEN").as_deref(), Some("abc"));
    assert_eq!(env.get("TOKEN").as_deref(), Some("abc"));
    let mut cmd = std::process::Command::new("x");
    let _cmd = env.apply_to(&mut cmd);
    let names: Vec<_> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
    assert_eq!(names, ["JG_TOKEN"], "exported once, not as JG_JG_TOKEN");
    // Changing it through either name changes the file's entry.
    env.set("TOKEN", "def").unwrap();
    assert_eq!(env.iter().collect::<Vec<_>>(), [("JG_TOKEN", "def")]);
    assert!(env.remove("TOKEN"));
    assert!(env.is_empty());
}

// --- performance ---------------------------------------------------------------

#[test]
fn test_bulk_operations_are_linear() {
    let mut text = String::new();
    for i in 0..60_000 {
        text.push_str(&format!("K{i}=v\n"));
    }
    let start = std::time::Instant::now();
    let mut env = Env::from_text(&text).unwrap();
    env.clear();
    for i in 0..60_000 {
        env.set(&format!("N{i}"), "v").unwrap();
    }
    for i in 0..60_000 {
        let _ = env.remove(&format!("N{i}"));
    }
    let elapsed = start.elapsed();
    // Quadratic bookkeeping took minutes here; linear takes well under a
    // second even unoptimized.
    assert!(elapsed.as_secs() < 30, "{elapsed:?}");
}

#[test]
fn test_save_of_many_changes_is_linear() {
    let sb = Sandbox::new("many");
    let mut env = Env::open(sb.file()).unwrap();
    for i in 0..30_000 {
        env.set(&format!("K{i}"), "v").unwrap();
    }
    env.save().unwrap();
    let start = std::time::Instant::now();
    env.clear();
    env.save().unwrap();
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    assert!(Env::open(sb.file()).unwrap().is_empty());
}

// --- file checks: every platform ---------------------------------------------------

#[test]
fn test_saved_file_passes_its_own_checks_and_is_private() {
    let sb = Sandbox::new("own");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("TOKEN", "x").unwrap();
    env.save().unwrap();
    path_lib::check_owner_private(sb.file()).unwrap();
    let again = Env::open(sb.file()).unwrap();
    assert_eq!(again.get("TOKEN").as_deref(), Some("x"));
}

#[test]
fn test_directory_at_the_path_is_an_io_error() {
    let sb = Sandbox::new("dir");
    fs::create_dir_all(sb.file()).unwrap();
    assert!(matches!(Env::open(sb.file()), Err(Error::Io { .. })));
    assert!(matches!(
        Env::open_unchecked(sb.file()),
        Err(Error::Io { .. })
    ));
}

#[test]
fn test_save_creates_missing_directories_private() {
    let sb = Sandbox::new("mkdir");
    let file = sb.dir.join("a").join("b").join(".env");
    let mut env = Env::open(&file).unwrap();
    env.set("K", "v").unwrap();
    env.save().unwrap();
    path_lib::check_owner_private(sb.dir.join("a")).unwrap();
    path_lib::check_owner_private(sb.dir.join("a").join("b")).unwrap();
}

#[test]
fn test_for_app_save_creates_a_private_file_in_a_checked_dir() {
    let sb = Sandbox::new("for-app");
    let app = path_lib::App::portable("EnvLibApp", &sb.dir).unwrap();
    let mut env = Env::for_app(&app).unwrap();
    env.set("A", "1").unwrap();
    env.save().unwrap();
    path_lib::check_owner(app.config(), path_lib::Others::MayRead).unwrap();
    path_lib::check_owner_private(app.config().join(".env")).unwrap();
    env.set("A", "2").unwrap();
    env.save().unwrap();
    env.reload().unwrap();
    assert_eq!(env.get("A").as_deref(), Some("2"));
}

// --- file checks: Unix ---------------------------------------------------------------

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn test_unix_file_others_can_write_is_refused() {
    let sb = Sandbox::new("unix-mode");
    fs::write(sb.file(), "DB_URL=postgres://attacker/\n").unwrap();
    set_mode(&sb.file(), 0o666);
    assert!(refused(Env::open(sb.file())).contains("write"));
    set_mode(&sb.file(), 0o620);
    let _ = refused(Env::open(sb.file()));
    // Readable by others is fine for a plain open.
    set_mode(&sb.file(), 0o644);
    let env = Env::open(sb.file()).unwrap();
    assert!(env.contains("DB_URL"));
    // The unchecked variant is for development setups.
    set_mode(&sb.file(), 0o666);
    assert!(Env::open_unchecked(sb.file()).unwrap().contains("DB_URL"));
    // A reload checks again.
    let mut env = env;
    assert!(matches!(env.reload(), Err(Error::Path(_))));
}

#[cfg(unix)]
#[test]
fn test_unix_fifo_is_refused_without_hanging() {
    let sb = Sandbox::new("unix-fifo");
    let made = std::process::Command::new("mkfifo").arg(sb.file()).status();
    if !made.is_ok_and(|s| s.success()) {
        return;
    }
    let file = sb.file();
    let (tx, rx) = std::sync::mpsc::channel();
    let _reader = std::thread::spawn(move || {
        let checked = matches!(Env::open(&file), Err(Error::Io { .. }));
        let unchecked = matches!(Env::open_unchecked(&file), Err(Error::Io { .. }));
        let _sent = tx.send((checked, unchecked));
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(20));
    assert_eq!(got.ok(), Some((true, true)), "a FIFO hung or was read");
}

#[cfg(unix)]
#[test]
fn test_unix_symlink_is_followed_only_from_a_safe_directory() {
    let sb = Sandbox::new("unix-link");
    let real = sb.dir.join("real.env");
    fs::write(&real, "A=1\n").unwrap();
    set_mode(&real, 0o600);
    std::os::unix::fs::symlink(&real, sb.file()).unwrap();
    assert!(Env::open(sb.file()).unwrap().contains("A"));
    // The same link in a directory other users can write is refused.
    let shared = sb.dir.join("shared");
    fs::create_dir(&shared).unwrap();
    set_mode(&shared, 0o777);
    std::os::unix::fs::symlink(&real, shared.join(".env")).unwrap();
    let _ = refused(Env::open(shared.join(".env")));
    set_mode(&shared, 0o700);
}

#[cfg(unix)]
#[test]
fn test_unix_saves_keep_a_wider_mode_and_stay_private_when_private() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("unix-save");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("A", "1").unwrap();
    env.save().unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&sb.file()), 0o600);
    set_mode(&sb.file(), 0o640);
    env.set("A", "2").unwrap();
    env.save().unwrap();
    assert_eq!(mode(&sb.file()), 0o640);
}

// --- file checks: Windows ---------------------------------------------------------------

#[cfg(windows)]
fn icacls(path: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "icacls {args:?}: {out:?}");
}

#[cfg(windows)]
#[test]
fn test_windows_file_others_can_write_is_refused() {
    let sb = Sandbox::new("win-write");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("DB_URL", "postgres://localhost/").unwrap();
    env.save().unwrap();
    icacls(&sb.file(), &["/grant", "*S-1-1-0:(M)"]);
    assert!(refused(Env::open(sb.file())).contains("change"));
    assert!(Env::open_unchecked(sb.file()).unwrap().contains("DB_URL"));
}

#[cfg(windows)]
#[test]
fn test_windows_global_env_is_private_and_refused_when_readable() {
    let sb = Sandbox::new("win-global");
    let root = sb.dir.join("svc");
    let app = path_lib::App::global("svc")
        .unwrap()
        .with_dir(path_lib::Kind::Data, &root)
        .unwrap()
        .with_dir(path_lib::Kind::Config, root.join("config"))
        .unwrap();
    let mut env = Env::for_app(&app).unwrap();
    env.set("DB_PASSWORD", "hunter2").unwrap();
    env.save().unwrap();
    let file = app.config().join(".env");
    let access = path_lib::file_access(&fs::File::open(&file).unwrap()).unwrap();
    assert!(
        access.owner_trusted && !access.others_can_read && !access.others_can_write,
        "{access:?}"
    );
    // The machine-wide config directory lets Users list it, not write it.
    path_lib::check_owner(app.config(), path_lib::Others::MayRead).unwrap();
    let again = Env::for_app(&app).unwrap();
    assert_eq!(again.get("DB_PASSWORD").as_deref(), Some("hunter2"));
    // A file other users can read is refused for a global application.
    icacls(&file, &["/grant", "*S-1-5-32-545:(R)"]);
    assert!(refused(Env::for_app(&app)).contains("read"));
    // A plain open only refuses what others can change.
    let _ = Env::open(&file).unwrap();
}

#[cfg(windows)]
#[test]
fn test_windows_planted_global_config_dir_is_refused_on_save() {
    let sb = Sandbox::new("win-planted");
    let root = sb.dir.join("svc");
    fs::create_dir_all(root.join("config")).unwrap();
    icacls(&root, &["/grant", "*S-1-5-11:(OI)(CI)(M)"]);
    let app = path_lib::App::global("svc")
        .unwrap()
        .with_dir(path_lib::Kind::Data, &root)
        .unwrap()
        .with_dir(path_lib::Kind::Config, root.join("config"))
        .unwrap();
    let mut env = Env::for_app(&app).unwrap();
    env.set("DB_PASSWORD", "hunter2").unwrap();
    assert!(matches!(
        env.save(),
        Err(Error::Path(path_lib::Error::Refused { .. }))
    ));
    assert!(!app.config().join(".env").exists());
}
