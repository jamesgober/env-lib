//! End-to-end persistence: the workflow `env-lib` exists for.

#![allow(clippy::unwrap_used, missing_docs)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use env_lib::{Env, Error};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(label: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join("env-lib-tests").join(format!(
            "{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join(".env")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_session_changes_persist_to_next_session() {
    let sb = Sandbox::new("sessions");
    // Session 1: create.
    let mut env = Env::open(sb.file()).unwrap();
    assert!(env.is_empty());
    env.set("PORT", "7000").unwrap();
    env.set("NAME", "hive db").unwrap();
    env.set("OLD", "x").unwrap();
    env.save().unwrap();
    assert!(!env.is_dirty());

    // Session 2: edit, delete, add.
    let mut env = Env::open(sb.file()).unwrap();
    assert_eq!(env.get("NAME").as_deref(), Some("hive db"));
    env.set("PORT", "7001").unwrap();
    assert!(env.remove("OLD"));
    env.set("NEW", "y").unwrap();
    env.save().unwrap();

    // Session 3: sees exactly what session 2 left.
    let env = Env::open(sb.file()).unwrap();
    assert_eq!(
        env.iter().collect::<Vec<_>>(),
        [("PORT", "7001"), ("NAME", "hive db"), ("NEW", "y")]
    );
}

#[test]
fn test_hand_edits_load_and_survive_saves() {
    let sb = Sandbox::new("hand-edit");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("A", "1").unwrap();
    env.save().unwrap();

    // The user edits the file in an editor.
    let edited = "# Database settings\nA=1\n\n# added by hand\nexport B = 'hand written'  # note\n";
    fs::write(sb.file(), edited).unwrap();

    let mut env = Env::open(sb.file()).unwrap();
    assert_eq!(env.get("B").as_deref(), Some("hand written"));
    env.set("C", "3").unwrap();
    env.save().unwrap();
    let saved = fs::read_to_string(sb.file()).unwrap();
    assert_eq!(
        saved,
        format!("{edited}C=3\n"),
        "hand-written lines kept verbatim"
    );
}

#[test]
fn test_reload_picks_up_external_edits_and_discards_unsaved() {
    let sb = Sandbox::new("reload");
    fs::write(sb.file(), "A=1\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    env.set("UNSAVED", "x").unwrap();
    fs::write(sb.file(), "A=2\nB=3\n").unwrap();
    env.reload().unwrap();
    assert_eq!(env.get("A").as_deref(), Some("2"));
    assert_eq!(env.get("B").as_deref(), Some("3"));
    assert!(!env.contains("UNSAVED"));
    assert!(!env.is_dirty());
}

#[test]
fn test_reload_keeps_custom_prefix() {
    let sb = Sandbox::new("reload-prefix");
    fs::write(sb.file(), "A=1\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap().with_prefix("APP_").unwrap();
    env.reload().unwrap();
    assert_eq!(env.prefix(), "APP_");
}

#[test]
fn test_system_variables_are_never_saved() {
    let sb = Sandbox::new("no-system");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("MINE", "1").unwrap();
    env.save().unwrap();
    assert_eq!(fs::read_to_string(sb.file()).unwrap(), "MINE=1\n");
}

#[test]
fn test_save_to_another_file_merges_with_that_file() {
    let sb = Sandbox::new("save-to");
    let other = sb.0.join("other.env");
    fs::write(&other, "# keep\nX=9\n").unwrap();
    let mut env = Env::from_text("A=1\n").unwrap();
    env.save_to(&other).unwrap();
    assert_eq!(fs::read_to_string(&other).unwrap(), "# keep\nX=9\nA=1\n");
    assert_eq!(
        env.get("X").as_deref(),
        Some("9"),
        "the target's variables are loaded too"
    );
    assert_eq!(env.file(), Some(other.as_path()));
}

#[test]
fn test_save_creates_missing_directories() {
    let sb = Sandbox::new("mkdirs");
    let deep = sb.0.join("a").join("b").join("c").join(".env");
    let mut env = Env::open(&deep).unwrap();
    env.set("A", "1").unwrap();
    env.save().unwrap();
    assert!(deep.is_file());
}

#[test]
fn test_crlf_files_keep_crlf() {
    let sb = Sandbox::new("crlf");
    fs::write(sb.file(), "A=1\r\nB=2\r\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    env.set("C", "3").unwrap();
    env.save().unwrap();
    assert_eq!(
        fs::read_to_string(sb.file()).unwrap(),
        "A=1\r\nB=2\r\nC=3\r\n"
    );
}

#[test]
fn test_malformed_file_reports_path_and_line() {
    let sb = Sandbox::new("bad");
    fs::write(sb.file(), "A=1\nB=2\noops\n").unwrap();
    match Env::open(sb.file()) {
        Err(Error::Syntax { path, line, .. }) => {
            assert_eq!(line, 3);
            assert_eq!(path.as_deref(), Some(sb.file().as_path()));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_invalid_utf8_reports_line() {
    let sb = Sandbox::new("utf8");
    fs::write(sb.file(), b"A=1\nB=\xff\n").unwrap();
    assert!(matches!(
        Env::open(sb.file()),
        Err(Error::Syntax { line: 2, .. })
    ));
}

#[test]
fn test_oversized_file_is_rejected() {
    let sb = Sandbox::new("big");
    let size = usize::try_from(env_lib::MAX_FILE_BYTES).unwrap() + 1;
    fs::write(sb.file(), vec![b'#'; size]).unwrap();
    assert!(matches!(Env::open(sb.file()), Err(Error::TooLarge { .. })));
}

#[test]
fn test_values_with_special_characters_round_trip_through_disk() {
    let sb = Sandbox::new("special");
    let values = [
        "",
        "a b",
        "\"q\"",
        "back\\slash",
        "$NOT_EXPANDED",
        "#hash",
        "line1\nline2",
        "tab\there",
        "名前",
        " lead",
        "trail ",
        "it's",
    ];
    let mut env = Env::open(sb.file()).unwrap();
    for (i, v) in values.iter().enumerate() {
        env.set(&format!("V{i}"), *v).unwrap();
    }
    env.save().unwrap();
    let env = Env::open(sb.file()).unwrap();
    for (i, v) in values.iter().enumerate() {
        assert_eq!(env.get(&format!("V{i}")).as_deref(), Some(*v), "{v:?}");
    }
}

#[test]
fn test_tilde_paths_expand_to_home() {
    let env = Env::open("~/env-lib-test-never-created/.env").unwrap();
    let home = path_lib::home().unwrap();
    assert!(env.file().unwrap().starts_with(home));
}

#[test]
fn test_for_app_uses_config_directory() {
    let sb = Sandbox::new("app");
    let app = path_lib::App::portable("EnvLibTest", &sb.0).unwrap();
    let mut env = Env::for_app(&app).unwrap();
    assert_eq!(env.file(), Some(app.config().join(".env").as_path()));
    env.set("A", "1").unwrap();
    env.save().unwrap();
    assert!(app.config().join(".env").is_file());
}

#[cfg(unix)]
#[test]
fn test_new_file_is_owner_only_and_existing_permissions_kept() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("perms");
    let mut env = Env::open(sb.file()).unwrap();
    env.set("SECRET", "x").unwrap();
    env.save().unwrap();
    let mode = fs::metadata(sb.file()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    fs::set_permissions(sb.file(), fs::Permissions::from_mode(0o640)).unwrap();
    env.set("SECRET", "y").unwrap();
    env.save().unwrap();
    let mode = fs::metadata(sb.file()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn test_save_failure_leaves_target_untouched() {
    let sb = Sandbox::new("save-fail");
    let target = sb.0.join("is-a-dir");
    fs::create_dir_all(&target).unwrap();
    let mut env = Env::new();
    env.set("A", "1").unwrap();
    assert!(env.save_to(&target).is_err());
    assert!(target.is_dir());
}

#[test]
fn test_unreadable_path_reports_io() {
    let sb = Sandbox::new("unreadable");
    // A directory where the file should be cannot be opened as a file.
    fs::create_dir_all(sb.file()).unwrap();
    let err = Env::open(sb.file()).unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
}

#[test]
fn test_hand_edits_made_while_running_survive_a_save() {
    let sb = Sandbox::new("concurrent-edit");
    fs::write(sb.file(), "KEEP=1\nSHARED=file\nGONE=1\nOURS=old\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    env.set("OURS", "program").unwrap();
    env.set("SHARED", "program").unwrap();
    env.set("NEW", "program").unwrap();

    // Meanwhile the user edits the file by hand.
    fs::write(
        sb.file(),
        "KEEP=1\nSHARED=hand\nADDED_BY_HAND=yes  # typed in an editor\nOURS=old\n",
    )
    .unwrap();

    env.save().unwrap();
    assert_eq!(
        fs::read_to_string(sb.file()).unwrap(),
        "KEEP=1\nSHARED=program\nADDED_BY_HAND=yes  # typed in an editor\nOURS=program\nNEW=program\n"
    );
    // The Env now reflects the file, including the hand edits.
    assert_eq!(env.get("ADDED_BY_HAND").as_deref(), Some("yes"));
    assert!(!env.contains("GONE"));
    assert!(!env.is_dirty());
}

#[test]
fn test_removal_this_session_applies_even_if_file_changed_meanwhile() {
    let sb = Sandbox::new("remove-vs-edit");
    fs::write(sb.file(), "A=1\nB=2\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    assert!(env.remove("A"));
    fs::write(sb.file(), "A=changed-by-hand\nB=2\nC=3\n").unwrap();
    env.save().unwrap();
    assert_eq!(fs::read_to_string(sb.file()).unwrap(), "B=2\nC=3\n");
}

#[test]
fn test_saving_without_changes_still_picks_up_hand_edits() {
    let sb = Sandbox::new("noop-save");
    fs::write(sb.file(), "A=1\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    fs::write(sb.file(), "A=2\n").unwrap();
    env.save().unwrap();
    assert_eq!(fs::read_to_string(sb.file()).unwrap(), "A=2\n");
    assert_eq!(env.get("A").as_deref(), Some("2"));
}

#[test]
fn test_changed_value_keeps_inline_comment() {
    let sb = Sandbox::new("inline-comment");
    fs::write(sb.file(), "PORT=7000   # http listener\n").unwrap();
    let mut env = Env::open(sb.file()).unwrap();
    env.set("PORT", "8080").unwrap();
    env.save().unwrap();
    assert_eq!(
        fs::read_to_string(sb.file()).unwrap(),
        "PORT=8080   # http listener\n"
    );
}
