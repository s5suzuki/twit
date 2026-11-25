use std::path::Path;
use std::process::Command;

use twit_core::git2::{Oid, Repository};
use twit_core::repo::{self, SeqOutcome};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "T"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn head_oid(dir: &Path) -> Oid {
    Repository::open(dir)
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
}

#[test]
fn merge_non_ff_creates_merge_commit() {
    let tmp = tempdir("non-ff");
    let dir = tmp.path();
    init(dir);
    write(dir, "base.txt", "base\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);

    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "feature.txt", "feat\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "feat"]);
    let feature = head_oid(dir);

    git(dir, &["checkout", "-q", "main"]);
    write(dir, "main.txt", "main\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "main-work"]);

    let out = repo::merge(dir, feature, "branch 'feature'").unwrap();
    assert!(matches!(out, SeqOutcome::Done));

    let repo = Repository::open(dir).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2, "should be a merge commit");
    assert_eq!(head.message().unwrap().trim(), "Merge branch 'feature'");
    assert!(dir.join("feature.txt").exists());
    assert!(repo::seq_state(dir) == repo::SeqState::None);
}

#[test]
fn merge_fast_forward_moves_head() {
    let tmp = tempdir("ff");
    let dir = tmp.path();
    init(dir);
    write(dir, "base.txt", "base\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);

    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "feature.txt", "feat\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "feat"]);
    let feature = head_oid(dir);

    git(dir, &["checkout", "-q", "main"]);
    let out = repo::merge(dir, feature, "branch 'feature'").unwrap();
    assert!(matches!(out, SeqOutcome::Done));

    let head = head_oid(dir);
    assert_eq!(head, feature, "fast-forward should move HEAD onto target");
    assert_eq!(
        Repository::open(dir)
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        1,
        "fast-forward must not create a merge commit"
    );
}

#[test]
fn merge_conflict_reports_files_and_aborts() {
    let tmp = tempdir("conflict");
    let dir = tmp.path();
    init(dir);
    write(dir, "shared.txt", "base\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);

    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "shared.txt", "feature side\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "feat"]);
    let feature = head_oid(dir);

    git(dir, &["checkout", "-q", "main"]);
    write(dir, "shared.txt", "main side\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "main-work"]);
    let before = head_oid(dir);

    let out = repo::merge(dir, feature, "branch 'feature'").unwrap();
    match out {
        SeqOutcome::Conflicts(files) => assert_eq!(files, vec!["shared.txt".to_string()]),
        SeqOutcome::Done => panic!("expected conflicts"),
    }
    assert!(repo::seq_state(dir) == repo::SeqState::Merge);

    repo::merge_abort(dir).unwrap();
    assert_eq!(head_oid(dir), before, "abort restores HEAD");
    assert!(repo::seq_state(dir) == repo::SeqState::None);
}

fn diverged(dir: &Path) -> Oid {
    init(dir);
    write(dir, "base.txt", "base\n");
    write(dir, "notes.txt", "keep\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);

    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "feature.txt", "feat\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "feat"]);
    let feature = head_oid(dir);

    git(dir, &["checkout", "-q", "main"]);
    write(dir, "main.txt", "main\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "main-work"]);
    feature
}

#[test]
fn dirty_tree_blocks_merge_cherry_pick_and_revert() {
    let tmp = tempdir("dirty");
    let dir = tmp.path();
    let feature = diverged(dir);
    let before = head_oid(dir);
    write(dir, "notes.txt", "keep\nunsaved work\n");

    assert!(repo::merge(dir, feature, "branch 'feature'").is_err());
    assert!(repo::cherry_pick(dir, feature).is_err());
    assert!(repo::revert(dir, before).is_err());

    assert_eq!(head_oid(dir), before);
    assert!(repo::seq_state(dir) == repo::SeqState::None);
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        "keep\nunsaved work\n"
    );
}

#[test]
fn untracked_files_do_not_block_merge() {
    let tmp = tempdir("untracked");
    let dir = tmp.path();
    let feature = diverged(dir);
    write(dir, "scratch.txt", "untracked\n");

    let out = repo::merge(dir, feature, "branch 'feature'").unwrap();
    assert!(matches!(out, SeqOutcome::Done));
    assert!(dir.join("scratch.txt").exists());
}

#[test]
fn fast_forward_is_allowed_with_a_dirty_tree() {
    let tmp = tempdir("ff-dirty");
    let dir = tmp.path();
    init(dir);
    write(dir, "notes.txt", "keep\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);
    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "feature.txt", "feat\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "feat"]);
    let feature = head_oid(dir);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "notes.txt", "keep\nunsaved work\n");

    repo::merge(dir, feature, "branch 'feature'").unwrap();
    assert_eq!(head_oid(dir), feature);
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        "keep\nunsaved work\n"
    );
}

#[test]
fn plain_commit_is_refused_while_merging() {
    let tmp = tempdir("commit-in-merge");
    let dir = tmp.path();
    init(dir);
    write(dir, "shared.txt", "base\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "base"]);
    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "shared.txt", "feature side\n");
    git(dir, &["commit", "-qam", "feat"]);
    let feature = head_oid(dir);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "shared.txt", "main side\n");
    git(dir, &["commit", "-qam", "main-work"]);
    let before = head_oid(dir);

    repo::merge(dir, feature, "branch 'feature'").unwrap();
    write(dir, "shared.txt", "resolved\n");
    repo::stage(dir, &["shared.txt".to_string()]).unwrap();

    assert!(repo::commit(dir, "resolve").is_err());
    assert_eq!(head_oid(dir), before);

    repo::merge_continue(dir).unwrap();
    let repo = Repository::open(dir).unwrap();
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        2
    );
}

fn remote_and_clone(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempdir(name);
    let up = tmp.path().join("up");
    let clone = tmp.path().join("clone");
    std::fs::create_dir_all(&up).unwrap();
    init(&up);
    write(&up, "f.txt", "0\n");
    git(&up, &["add", "."]);
    git(&up, &["commit", "-qm", "init"]);
    git(
        tmp.path(),
        &["clone", "-q", up.to_str().unwrap(), clone.to_str().unwrap()],
    );
    git(&clone, &["config", "user.email", "t@example.com"]);
    git(&clone, &["config", "user.name", "T"]);

    git(&up, &["checkout", "-qb", "aaa-experimental"]);
    write(&up, "exp.txt", "exp\n");
    git(&up, &["add", "."]);
    git(&up, &["commit", "-qm", "experimental"]);
    git(&up, &["checkout", "-q", "main"]);
    (tmp, up, clone)
}

#[test]
fn pull_merges_only_the_upstream_branch() {
    let (_tmp, up, clone) = remote_and_clone("pull-upstream");
    write(&up, "f.txt", "1\n");
    git(&up, &["commit", "-qam", "main moves"]);
    let up_main = head_oid(&up);

    let out = repo::pull(&clone, "origin", |_, _| {}).unwrap();
    assert!(matches!(out, SeqOutcome::Done));
    assert_eq!(head_oid(&clone), up_main);
    assert!(!clone.join("exp.txt").exists());
}

#[test]
fn pull_without_upstream_is_refused() {
    let (_tmp, _up, clone) = remote_and_clone("pull-no-upstream");
    git(&clone, &["checkout", "-qb", "feature"]);
    let before = head_oid(&clone);

    let err = repo::pull(&clone, "origin", |_, _| {}).err().unwrap();
    assert!(err.message().contains("no upstream"), "{err}");
    assert_eq!(head_oid(&clone), before);
    assert!(!clone.join("exp.txt").exists());
}

fn tempdir(name: &str) -> TempDir {
    let base = std::env::temp_dir().join(format!("twit-merge-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    TempDir { path: base }
}

struct TempDir {
    path: std::path::PathBuf,
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
