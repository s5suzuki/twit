use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use twit_core::repo::{self, DiffMode, DiffRow, FileDiff, LineKind};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempRepo(PathBuf);

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap()
}

fn repo_with(file: &str, committed: &str) -> TempRepo {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("twit-partial-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["config", "user.name", "T"]);
    std::fs::write(dir.join("keep.txt"), "keep\n").unwrap();
    if !committed.is_empty() {
        std::fs::write(dir.join(file), committed).unwrap();
    }
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "init"]);
    TempRepo(dir)
}

fn index_of(dir: &Path, file: &str) -> String {
    git(dir, &["show", &format!(":{file}")])
}

fn diff(dir: &Path, file: &str, mode: DiffMode) -> FileDiff {
    repo::file_diff(dir, file, None, mode).unwrap()
}

fn changes(d: &FileDiff) -> Vec<usize> {
    d.rows
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, DiffRow::Line { kind, .. } if *kind != LineKind::Context))
        .map(|(i, _)| i)
        .collect()
}

const PAIRS: &[(&str, &str)] = &[
    ("a\nb\nc\n", "A\nb\nC\n"),
    ("one\ntwo\nthree", "ONE\ntwo\nTHREE"),
    ("a", "a\n"),
    ("a\n", "a"),
    ("a\nb", "a\n"),
    ("a\n", "a\nb"),
    ("x\r\ny\r\n", "x\r\nY\r\n"),
    ("\n", "\n\n"),
];

#[test]
fn stage_unstage_and_discard_everything_round_trips() {
    for (old, new) in PAIRS {
        let t = repo_with("f.txt", old);
        let dir = &t.0;
        std::fs::write(dir.join("f.txt"), new).unwrap();

        let d = diff(dir, "f.txt", DiffMode::Unstaged);
        let all = changes(&d);
        assert!(!all.is_empty(), "{old:?} -> {new:?}");
        repo::apply_partial(dir, "f.txt", &d, 0, d.rows.len() - 1, false)
            .unwrap_or_else(|e| panic!("stage {old:?} -> {new:?}: {e}"));
        assert_eq!(index_of(dir, "f.txt"), *new, "stage {old:?} -> {new:?}");

        let d = diff(dir, "f.txt", DiffMode::Staged);
        repo::apply_partial(dir, "f.txt", &d, 0, d.rows.len() - 1, true)
            .unwrap_or_else(|e| panic!("unstage {old:?} -> {new:?}: {e}"));
        assert_eq!(index_of(dir, "f.txt"), *old, "unstage {old:?} -> {new:?}");

        let d = diff(dir, "f.txt", DiffMode::Unstaged);
        repo::discard_partial(dir, "f.txt", &d, 0, d.rows.len() - 1)
            .unwrap_or_else(|e| panic!("discard {old:?} -> {new:?}: {e}"));
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            *old,
            "discard {old:?} -> {new:?}"
        );
    }
}

#[test]
fn single_line_operations_without_trailing_newline() {
    let t = repo_with("f.txt", "one\ntwo\nthree");
    let dir = &t.0;
    std::fs::write(dir.join("f.txt"), "ONE\ntwo\nTHREE").unwrap();

    let d = diff(dir, "f.txt", DiffMode::Unstaged);
    assert!(
        !d.rows
            .iter()
            .any(|r| matches!(r, DiffRow::Line { left: Some(l), .. } if l.contains("No newline"))),
        "EOF marker must not become a row"
    );
    let first = changes(&d)[0];
    repo::apply_partial(dir, "f.txt", &d, first, first, false).unwrap();
    assert_eq!(index_of(dir, "f.txt"), "ONE\ntwo\nthree");

    let d = diff(dir, "f.txt", DiffMode::Staged);
    let first = changes(&d)[0];
    repo::apply_partial(dir, "f.txt", &d, first, first, true).unwrap();
    assert_eq!(index_of(dir, "f.txt"), "one\ntwo\nthree");

    let d = diff(dir, "f.txt", DiffMode::Unstaged);
    let last = *changes(&d).last().unwrap();
    repo::discard_partial(dir, "f.txt", &d, last, last).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "ONE\ntwo\nthree"
    );
}

#[test]
fn discard_refuses_when_the_file_changed_after_the_diff_was_loaded() {
    let t = repo_with("f.txt", "a\nb\n");
    let dir = &t.0;
    std::fs::write(dir.join("f.txt"), "A\nb\n").unwrap();
    let d = diff(dir, "f.txt", DiffMode::Unstaged);
    std::fs::write(dir.join("f.txt"), "A\nb\nnewer edit\n").unwrap();

    let first = changes(&d)[0];
    assert!(repo::discard_partial(dir, "f.txt", &d, first, first).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "A\nb\nnewer edit\n"
    );
}

#[test]
fn untracked_file_can_be_staged_line_by_line() {
    let t = repo_with("unused.txt", "");
    let dir = &t.0;
    std::fs::write(dir.join("new.txt"), "a\nb\n").unwrap();

    let d = diff(dir, "new.txt", DiffMode::Unstaged);
    let first = changes(&d)[0];
    repo::apply_partial(dir, "new.txt", &d, first, first, false).unwrap();
    assert_eq!(index_of(dir, "new.txt"), "a\n");

    let d = diff(dir, "new.txt", DiffMode::Unstaged);
    repo::apply_partial(dir, "new.txt", &d, 0, d.rows.len() - 1, false).unwrap();
    assert_eq!(index_of(dir, "new.txt"), "a\nb\n");
}

#[test]
fn untracked_executable_keeps_its_mode() {
    let t = repo_with("unused.txt", "");
    let dir = &t.0;
    let script = dir.join("run.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let d = diff(dir, "run.sh", DiffMode::Unstaged);
    let first = changes(&d)[0];
    repo::apply_partial(dir, "run.sh", &d, first, first, false).unwrap();
    assert!(
        git(dir, &["ls-files", "-s", "run.sh"]).starts_with("100755"),
        "executable bit must be kept"
    );
}

#[test]
fn rename_header_uses_the_old_path_it_is_given() {
    let t = repo_with("a.txt", "1\n2\n3\n4\n5\n");
    let dir = &t.0;
    git(dir, &["mv", "a.txt", "b.txt"]);

    let d = repo::file_diff(dir, "b.txt", Some("a.txt"), DiffMode::Staged).unwrap();
    assert!(d.rename);
    assert!(matches!(&d.rows[0], DiffRow::FileHeader(h) if h == "a.txt  →  b.txt"));
}
