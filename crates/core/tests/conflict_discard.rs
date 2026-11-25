use std::path::{Path, PathBuf};
use std::process::Command;

use twit_core::repo::{self, SeqOutcome, StatusKind};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_ALLOW_PROTOCOL", "file")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn git_try(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_ALLOW_PROTOCOL", "file")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "T"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "protocol.file.allow", "always"]);
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(name: &str) -> TempDir {
    let base = std::env::temp_dir().join(format!("twit-conf-disc-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    TempDir { path: base }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

struct Fixture {
    _tmp: TempDir,
    parent: PathBuf,
    ours_sub: String,
}

fn setup(name: &str) -> Fixture {
    let f = build(name);
    assert!(
        !git_try(&f.parent, &["rebase", "main"]),
        "rebase was expected to stop on conflicts"
    );
    f
}

fn build(name: &str) -> Fixture {
    let tmp = tempdir(name);
    let root = tmp.path().to_path_buf();

    let origin = root.join("sub_origin");
    std::fs::create_dir_all(&origin).unwrap();
    init(&origin);
    write(&origin.join("f.txt"), "v1\n");
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-qm", "c1"]);
    let base = git_out(&origin, &["rev-parse", "HEAD"]);
    write(&origin.join("f.txt"), "ours\n");
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-qm", "ours"]);
    let sub_ours = git_out(&origin, &["rev-parse", "HEAD"]);
    git(&origin, &["checkout", "-q", &base]);
    write(&origin.join("f.txt"), "theirs\n");
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-qm", "theirs"]);
    let sub_theirs = git_out(&origin, &["rev-parse", "HEAD"]);
    git(&origin, &["checkout", "-q", "-B", "main", &base]);

    let parent = root.join("parent");
    std::fs::create_dir_all(&parent).unwrap();
    init(&parent);
    write(&parent.join("keep.txt"), "keep\n");
    write(&parent.join("conf.txt"), "shared\n");
    write(&parent.join("gone.txt"), "gone\n");
    git(&parent, &["add", "."]);
    git(&parent, &["commit", "-qm", "base"]);
    git(&parent, &["submodule", "add", "-q", "../sub_origin", "sub"]);
    git(&parent.join("sub"), &["checkout", "-q", &base]);
    git(&parent, &["add", "sub"]);
    git(&parent, &["commit", "-qm", "sub at base"]);

    git(&parent, &["checkout", "-q", "-b", "feature"]);
    git(&parent.join("sub"), &["checkout", "-q", &sub_theirs]);
    write(&parent.join("conf.txt"), "theirs\n");
    write(&parent.join("gone.txt"), "modified by them\n");
    git(&parent, &["add", "sub", "conf.txt", "gone.txt"]);
    git(&parent, &["commit", "-qm", "theirs"]);

    git(&parent, &["checkout", "-q", "main"]);
    git(&parent.join("sub"), &["checkout", "-q", &sub_ours]);
    write(&parent.join("conf.txt"), "ours\n");
    git(&parent, &["rm", "-q", "gone.txt"]);
    git(&parent, &["add", "sub", "conf.txt"]);
    git(&parent, &["commit", "-qm", "ours"]);

    git(&parent, &["checkout", "-q", "feature"]);
    let recorded = git_out(&parent, &["rev-parse", "HEAD:sub"]);
    git(&parent.join("sub"), &["checkout", "-q", &recorded]);

    Fixture {
        _tmp: tmp,
        parent,
        ours_sub: sub_ours,
    }
}

#[test]
fn conflicted_submodule_is_listed_once() {
    let f = setup("listed-once");
    let (_, unstaged) = repo::load_status(&f.parent).unwrap();
    let rows: Vec<&str> = unstaged
        .iter()
        .filter(|e| e.path.trim_end_matches('/') == "sub")
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(rows, vec!["sub"]);
    assert!(
        unstaged
            .iter()
            .any(|e| e.path == "sub" && e.kind == StatusKind::Conflicted)
    );
}

#[test]
fn discard_resolves_a_conflicted_submodule_to_head() {
    let f = setup("submodule");
    repo::discard(&f.parent, &["sub".to_string()]).unwrap();

    assert!(!git_out(&f.parent, &["status", "--porcelain"]).contains(" sub"));
    assert_eq!(
        git_out(&f.parent.join("sub"), &["rev-parse", "HEAD"]),
        f.ours_sub
    );
    assert_eq!(git_out(&f.parent, &["rev-parse", "HEAD:sub"]), f.ours_sub);
    assert!(!repo::seq_conflicts(&f.parent).contains(&"sub".to_string()));
}

#[test]
fn discard_resolves_a_conflicted_file_to_head() {
    let f = setup("file");
    repo::discard(&f.parent, &["conf.txt".to_string()]).unwrap();

    assert_eq!(
        std::fs::read_to_string(f.parent.join("conf.txt")).unwrap(),
        "ours\n"
    );
    assert!(!repo::seq_conflicts(&f.parent).contains(&"conf.txt".to_string()));
    assert!(!git_out(&f.parent, &["status", "--porcelain"]).contains("conf.txt"));
}

#[test]
fn discard_removes_a_conflicted_path_missing_from_head() {
    let f = setup("deleted");
    repo::discard(&f.parent, &["gone.txt".to_string()]).unwrap();

    assert!(!f.parent.join("gone.txt").exists());
    assert!(!repo::seq_conflicts(&f.parent).contains(&"gone.txt".to_string()));
    assert!(!git_out(&f.parent, &["status", "--porcelain"]).contains("gone.txt"));
}

#[test]
fn staging_a_conflicted_submodule_marks_it_resolved() {
    let f = setup("stage");
    let wd = git_out(&f.parent.join("sub"), &["rev-parse", "HEAD"]);
    repo::stage(&f.parent, &["sub".to_string()]).unwrap();

    assert!(!repo::seq_conflicts(&f.parent).contains(&"sub".to_string()));
    assert_eq!(git_out(&f.parent, &["rev-parse", ":sub"]), wd);
}

#[test]
fn discard_leaves_other_conflicts_alone() {
    let f = setup("scoped");
    repo::discard(&f.parent, &["conf.txt".to_string()]).unwrap();
    assert_eq!(repo::seq_conflicts(&f.parent), vec!["gone.txt", "sub"]);
}

#[test]
fn discarding_a_submodule_lets_an_own_rebase_continue() {
    let f = build("continue");
    let onto = git2::Oid::from_str(&git_out(&f.parent, &["rev-parse", "main"])).unwrap();
    match repo::rebase_onto(&f.parent, onto) {
        Ok(SeqOutcome::Conflicts(c)) => assert!(c.contains(&"sub".to_string())),
        Ok(SeqOutcome::Done) => panic!("rebase was expected to stop on conflicts"),
        Err(e) => panic!("rebase_onto: {e}"),
    }

    repo::discard(&f.parent, &["sub".to_string()]).unwrap();
    repo::discard(&f.parent, &["gone.txt".to_string()]).unwrap();
    repo::stage(&f.parent, &["conf.txt".to_string()]).unwrap();
    assert!(repo::seq_conflicts(&f.parent).is_empty());

    match repo::rebase_continue(&f.parent) {
        Ok(SeqOutcome::Done) => {}
        Ok(SeqOutcome::Conflicts(c)) => panic!("unexpected conflicts: {c:?}"),
        Err(e) => panic!("rebase_continue: {e}"),
    }
    assert_eq!(git_out(&f.parent, &["status", "--porcelain"]), "");
    assert_eq!(git_out(&f.parent, &["rev-parse", "HEAD:sub"]), f.ours_sub);
}
