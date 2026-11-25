use std::path::Path;
use std::process::Command;

use twit_core::git2::Repository;
use twit_core::repo;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn init(dir: &Path, remote: &str) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "fallback@example.com"]);
    git(dir, &["config", "user.name", "Fallback"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["remote", "add", "origin", remote]);
    let account = dir.join("account.gitconfig");
    std::fs::write(
        &account,
        "[user]\n\tname = Acme User\n\temail = user@acme.example\n",
    )
    .unwrap();
    git(
        dir,
        &[
            "config",
            "includeIf.hasconfig:remote.*.url:**/acme/**.path",
            account.to_str().unwrap(),
        ],
    );
}

fn head_author(dir: &Path) -> (String, String) {
    let repo = Repository::open(dir).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    let author = commit.author();
    (
        author.name().unwrap().to_string(),
        author.email().unwrap().to_string(),
    )
}

fn commit_file(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), "body\n").unwrap();
    git(dir, &["add", name]);
    repo::commit(dir, "test commit").unwrap();
}

#[test]
fn commit_identity_matches_git_when_hasconfig_include_applies() {
    let tmp = tempdir("match");
    let dir = tmp.path();
    init(dir, "https://github.com/acme/widget.git");

    let expected = git_out(dir, &["config", "user.email"]);
    assert_eq!(expected, "user@acme.example", "git resolves the include");

    commit_file(dir, "a.txt");
    assert_eq!(
        head_author(dir),
        ("Acme User".to_string(), "user@acme.example".to_string())
    );
}

#[test]
fn commit_identity_falls_back_when_no_remote_url_matches() {
    let tmp = tempdir("nomatch");
    let dir = tmp.path();
    init(dir, "https://github.com/other/widget.git");

    let expected = git_out(dir, &["config", "user.email"]);
    assert_eq!(expected, "fallback@example.com", "git skips the include");

    commit_file(dir, "a.txt");
    assert_eq!(
        head_author(dir),
        ("Fallback".to_string(), "fallback@example.com".to_string())
    );
}

fn tempdir(name: &str) -> TempDir {
    let base =
        std::env::temp_dir().join(format!("twit-identity-test-{}-{name}", std::process::id()));
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
