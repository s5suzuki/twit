use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use git2::{
    ApplyLocation, ApplyOptions, Cred, CredentialType, Diff, DiffOptions, ErrorCode, FetchOptions,
    Index, IndexAddOption, Oid, PushOptions, Rebase, RemoteCallbacks, Repository, RepositoryState,
    Signature, Submodule, SubmoduleUpdateOptions,
};

use super::gitconfig;

pub fn stage(repo_path: &Path, paths: &[String]) -> Result<(), git2::Error> {
    if paths.is_empty() {
        return Ok(());
    }
    let repo = Repository::open(repo_path)?;
    let rest = stage_conflicted_submodules(&repo, paths)?;
    if rest.is_empty() {
        return Ok(());
    }
    let mut index = repo.index()?;
    index.add_all(rest.iter(), IndexAddOption::DEFAULT, None)?;
    index.write()
}

fn stage_conflicted_submodules<'a>(
    repo: &Repository,
    paths: &'a [String],
) -> Result<Vec<&'a str>, git2::Error> {
    let mut rest = Vec::new();
    let mut resolved = Vec::new();
    {
        let index = repo.index()?;
        for p in paths {
            let trimmed = p.trim_end_matches('/');
            if index.get_path(Path::new(trimmed), 0).is_none()
                && let Some(sm) = find_submodule_by_path(repo, trimmed)
                && let Some(oid) = submodule_workdir_id(&sm)
            {
                resolved.push((trimmed, oid));
                continue;
            }
            rest.push(p.as_str());
        }
    }
    if resolved.is_empty() {
        return Ok(rest);
    }

    let mut index = repo.index()?;
    for (path, oid) in resolved {
        index.conflict_remove(Path::new(path))?;
        index.add(&index_entry(path, oid, 0o160000))?;
    }
    index.write()?;
    Ok(rest)
}

fn index_entry(path: &str, id: Oid, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

fn submodule_workdir_id(sm: &Submodule) -> Option<Oid> {
    let from_head = sm
        .open()
        .and_then(|r| Ok(r.head()?.target()))
        .ok()
        .flatten();
    from_head.or_else(|| sm.workdir_id())
}

pub fn unstage(repo_path: &Path, paths: &[String]) -> Result<(), git2::Error> {
    if paths.is_empty() {
        return Ok(());
    }
    let repo = Repository::open(repo_path)?;
    match repo.head() {
        Ok(head) => {
            let obj = head.peel(git2::ObjectType::Commit)?;
            repo.reset_default(Some(&obj), paths.iter().map(String::as_str))
        }
        Err(_) => {
            let mut index = repo.index()?;
            for p in paths {
                index.remove_path(Path::new(p))?;
            }
            index.write()
        }
    }
}

pub fn stage_hunk(repo_path: &Path, file: &str, hunk_index: usize) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(file);
    opts.context_lines(0);
    let diff = repo.diff_index_to_workdir(None, Some(&mut opts))?;
    apply_nth_hunk(&repo, &diff, hunk_index)
}

pub fn unstage_hunk(repo_path: &Path, file: &str, hunk_index: usize) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.pathspec(file);
    opts.context_lines(0);
    opts.reverse(true);
    let diff = repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?;
    apply_nth_hunk(&repo, &diff, hunk_index)
}

fn apply_nth_hunk(repo: &Repository, diff: &Diff, target: usize) -> Result<(), git2::Error> {
    let mut counter = 0usize;
    let mut opts = ApplyOptions::new();
    opts.hunk_callback(|_hunk| {
        let take = counter == target;
        counter += 1;
        take
    });
    repo.apply(diff, ApplyLocation::Index, Some(&mut opts))
}

pub fn discard(repo_path: &Path, paths: &[String]) -> Result<(), git2::Error> {
    if paths.is_empty() {
        return Ok(());
    }
    let repo = Repository::open(repo_path)?;
    resolve_conflicts_to_head(&repo, paths)?;

    let mut file_paths = Vec::new();
    for p in paths {
        if !reset_submodule_pointer(&repo, p)? {
            file_paths.push(p);
        }
    }

    if file_paths.is_empty() {
        return Ok(());
    }
    let mut cb = git2::build::CheckoutBuilder::new();
    cb.force().remove_untracked(true);
    for p in &file_paths {
        cb.path(p);
    }
    repo.checkout_index(None, Some(&mut cb))
}

fn resolve_conflicts_to_head(repo: &Repository, paths: &[String]) -> Result<(), git2::Error> {
    let conflicted = {
        let index = repo.index()?;
        if !index.has_conflicts() {
            return Ok(());
        }
        conflict_paths(&index)
    };
    let targets: Vec<&str> = paths
        .iter()
        .map(|p| p.trim_end_matches('/'))
        .filter(|p| conflicted.iter().any(|c| c == p))
        .collect();
    if targets.is_empty() {
        return Ok(());
    }

    let head_tree = repo.head().and_then(|h| h.peel_to_tree()).ok();
    let mut index = repo.index()?;
    for t in &targets {
        let p = Path::new(t);
        index.conflict_remove(p)?;
        match head_tree.as_ref().and_then(|tree| tree.get_path(p).ok()) {
            Some(entry) => index.add(&index_entry(t, entry.id(), entry.filemode() as u32))?,
            None => {
                let _ = index.remove_path(p);
            }
        }
    }
    index.write()
}

pub(crate) fn reset_submodule_pointer(repo: &Repository, path: &str) -> Result<bool, git2::Error> {
    match find_submodule_by_path(repo, path) {
        Some(sm) => {
            discard_submodule(&sm)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn find_submodule_by_path<'a>(repo: &'a Repository, path: &str) -> Option<Submodule<'a>> {
    let target = path.trim_end_matches('/');
    repo.submodules().ok()?.into_iter().find(|sm| {
        sm.path()
            .to_str()
            .is_some_and(|p| p.trim_end_matches('/') == target)
    })
}

fn discard_submodule(sm: &Submodule) -> Result<(), git2::Error> {
    let target = sm
        .index_id()
        .or_else(|| sm.head_id())
        .ok_or_else(|| git2::Error::from_str("submodule has no recorded commit to restore"))?;
    let sub_repo = sm.open()?;
    let obj = sub_repo.find_object(target, None)?;
    let mut cb = git2::build::CheckoutBuilder::new();
    cb.force().remove_untracked(true);
    sub_repo.checkout_tree(&obj, Some(&mut cb))?;
    sub_repo.set_head_detached(target)
}

pub fn checkout_branch(repo_path: &Path, branch: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let refname = format!("refs/heads/{branch}");
    let obj = repo.revparse_single(&refname)?;
    repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))?;
    repo.set_head(&refname)
}

pub fn checkout_commit(repo_path: &Path, oid: Oid) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let obj = repo.find_object(oid, None)?;
    repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))?;
    repo.set_head_detached(oid)
}

pub enum SeqOutcome {
    Done,
    Conflicts(Vec<String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SeqState {
    None,
    Rebase,
    RebaseInteractive,
    CherryPick,
    Revert,
    Merge,
}

pub fn seq_state(repo_path: &Path) -> SeqState {
    match Repository::open(repo_path).map(|r| r.state()) {
        Ok(RepositoryState::RebaseInteractive) => SeqState::RebaseInteractive,
        Ok(RepositoryState::Rebase) | Ok(RepositoryState::RebaseMerge) => SeqState::Rebase,
        Ok(RepositoryState::CherryPick) | Ok(RepositoryState::CherryPickSequence) => {
            SeqState::CherryPick
        }
        Ok(RepositoryState::Revert) | Ok(RepositoryState::RevertSequence) => SeqState::Revert,
        Ok(RepositoryState::Merge) => SeqState::Merge,
        _ => SeqState::None,
    }
}

pub fn seq_conflicts(repo_path: &Path) -> Vec<String> {
    match Repository::open(repo_path).and_then(|r| r.index()) {
        Ok(index) => conflict_paths(&index),
        Err(_) => Vec::new(),
    }
}

pub fn commit_parent_count(repo_path: &Path, oid: Oid) -> Result<usize, git2::Error> {
    let repo = Repository::open(repo_path)?;
    Ok(repo.find_commit(oid)?.parent_count())
}

pub fn rebase_onto(repo_path: &Path, onto_oid: Oid) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let head = repo.head()?;
    let branch = repo.reference_to_annotated_commit(&head)?;
    let onto = repo.find_annotated_commit(onto_oid)?;

    let mut rebase = repo.rebase(Some(&branch), Some(&onto), None, None)?;
    drive_rebase(&repo, &mut rebase)
}

pub fn rebase_continue(repo_path: &Path) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut rebase = repo.open_rebase(None)?;
    if repo.index()?.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&repo.index()?)));
    }

    let sig = gitconfig::signature(&repo)?;
    commit_op(&mut rebase, &sig)?;
    drive_rebase(&repo, &mut rebase)
}

pub fn rebase_abort(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut rebase = repo.open_rebase(None)?;
    rebase.abort()
}

pub fn cherry_pick(repo_path: &Path, oid: Oid) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.find_commit(oid)?;
    let mut opts = git2::CherrypickOptions::new();
    if commit.parent_count() > 1 {
        opts.mainline(1);
    }
    repo.cherrypick(&commit, Some(&mut opts))?;
    finish_cherry_pick(&repo, &commit)
}

pub fn cherry_pick_continue(repo_path: &Path) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    if repo.index()?.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&repo.index()?)));
    }
    let pick_oid = pseudo_ref_oid(&repo, "CHERRY_PICK_HEAD")?;
    let commit = repo.find_commit(pick_oid)?;
    finish_cherry_pick(&repo, &commit)
}

pub fn cherry_pick_abort(repo_path: &Path) -> Result<(), git2::Error> {
    reset_hard_to_head(repo_path)
}

fn pseudo_ref_oid(repo: &Repository, name: &str) -> Result<Oid, git2::Error> {
    let raw = std::fs::read_to_string(repo.path().join(name))
        .map_err(|e| git2::Error::from_str(&format!("read {name}: {e}")))?;
    Oid::from_str(raw.trim())
}

pub fn revert(repo_path: &Path, oid: Oid) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.find_commit(oid)?;
    let mut opts = git2::RevertOptions::new();
    if commit.parent_count() > 1 {
        opts.mainline(1);
    }
    repo.revert(&commit, Some(&mut opts))?;
    finish_revert(&repo, &commit)
}

pub fn revert_continue(repo_path: &Path) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    if repo.index()?.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&repo.index()?)));
    }
    let reverted = repo.find_commit(pseudo_ref_oid(&repo, "REVERT_HEAD")?)?;
    finish_revert(&repo, &reverted)
}

pub fn revert_abort(repo_path: &Path) -> Result<(), git2::Error> {
    reset_hard_to_head(repo_path)
}

fn finish_revert(repo: &Repository, reverted: &git2::Commit) -> Result<SeqOutcome, git2::Error> {
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head()?.peel_to_commit()?;
    let sig = gitconfig::signature(repo)?;
    let subject = reverted.summary().ok().flatten().unwrap_or("commit");
    let msg = format!(
        "Revert \"{subject}\"\n\nThis reverts commit {}.\n",
        reverted.id()
    );
    repo.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&head])?;
    repo.cleanup_state()?;
    Ok(SeqOutcome::Done)
}

fn reset_hard_to_head(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let head = repo.head()?.peel_to_commit()?;
    repo.reset(head.as_object(), git2::ResetType::Hard, None)?;
    repo.cleanup_state()
}

pub fn merge_continue(repo_path: &Path) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
    }
    let their_oid = pseudo_ref_oid(&repo, "MERGE_HEAD")?;
    let their = repo.find_commit(their_oid)?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head()?.peel_to_commit()?;
    let sig = gitconfig::signature(&repo)?;
    let msg = repo
        .message()
        .unwrap_or_else(|_| format!("Merge commit '{their_oid}'"));
    repo.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&head, &their])?;
    repo.cleanup_state()?;
    Ok(SeqOutcome::Done)
}

pub fn merge_abort(repo_path: &Path) -> Result<(), git2::Error> {
    reset_hard_to_head(repo_path)
}

pub fn merge(repo_path: &Path, oid: Oid, label: &str) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let annotated = repo.find_annotated_commit(oid)?;
    let analysis = repo.merge_analysis(&[&annotated])?.0;

    if analysis.is_up_to_date() {
        return Ok(SeqOutcome::Done);
    }

    if analysis.is_fast_forward() {
        let obj = repo.find_object(oid, None)?;
        repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))?;
        let refname = repo.head()?.name().map(String::from)?;
        repo.find_reference(&refname)?
            .set_target(oid, "merge: fast-forward")?;
        return Ok(SeqOutcome::Done);
    }

    repo.merge(&[&annotated], None, None)?;
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head()?.peel_to_commit()?;
    let their = repo.find_commit(oid)?;
    let sig = gitconfig::signature(&repo)?;
    let msg = format!("Merge {label}");
    repo.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&head, &their])?;
    repo.cleanup_state()?;
    Ok(SeqOutcome::Done)
}

fn finish_cherry_pick(repo: &Repository, picked: &git2::Commit) -> Result<SeqOutcome, git2::Error> {
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head()?.peel_to_commit()?;
    let committer = gitconfig::signature(repo)?;

    repo.commit(
        Some("HEAD"),
        &picked.author(),
        &committer,
        picked.message().unwrap_or(""),
        &tree,
        &[&head],
    )?;
    repo.cleanup_state()?;
    Ok(SeqOutcome::Done)
}

pub fn create_branch(repo_path: &Path, name: &str, oid: Oid) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.find_commit(oid)?;
    repo.branch(name, &commit, false)?;
    Ok(())
}

pub fn rename_branch(repo_path: &Path, from: &str, to: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut branch = repo.find_branch(from, git2::BranchType::Local)?;
    branch.rename(to, false)?;
    Ok(())
}

pub fn delete_branch(repo_path: &Path, name: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut branch = repo.find_branch(name, git2::BranchType::Local)?;
    branch.delete()
}

pub fn create_tag(repo_path: &Path, name: &str, oid: Oid) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let obj = repo.find_object(oid, None)?;
    repo.tag_lightweight(name, &obj, false)?;
    Ok(())
}

pub fn delete_tag(repo_path: &Path, name: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    repo.tag_delete(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn label(self) -> &'static str {
        match self {
            ResetMode::Soft => "Soft",
            ResetMode::Mixed => "Mixed",
            ResetMode::Hard => "Hard",
        }
    }
}

pub fn reset(repo_path: &Path, oid: Oid, mode: ResetMode) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let obj = repo.find_object(oid, None)?;
    let kind = match mode {
        ResetMode::Soft => git2::ResetType::Soft,
        ResetMode::Mixed => git2::ResetType::Mixed,
        ResetMode::Hard => git2::ResetType::Hard,
    };
    repo.reset(&obj, kind, None)
}

pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub oid: Oid,
}

pub fn stash_save(repo_path: &Path, message: Option<&str>) -> Result<(), git2::Error> {
    let mut repo = Repository::open(repo_path)?;
    let sig = gitconfig::signature(&repo)?;
    repo.stash_save2(&sig, message, Some(git2::StashFlags::INCLUDE_UNTRACKED))?;
    Ok(())
}

pub fn stash_list(repo_path: &Path) -> Vec<StashEntry> {
    let mut out = Vec::new();
    if let Ok(mut repo) = Repository::open(repo_path) {
        let _ = repo.stash_foreach(|index, message, oid| {
            out.push(StashEntry {
                index,
                message: message.to_string(),
                oid: *oid,
            });
            true
        });
    }
    out
}

pub fn stash_pop(repo_path: &Path, index: usize) -> Result<(), git2::Error> {
    Repository::open(repo_path)?.stash_pop(index, None)
}

pub fn stash_apply(repo_path: &Path, index: usize) -> Result<(), git2::Error> {
    Repository::open(repo_path)?.stash_apply(index, None)
}

pub fn stash_drop(repo_path: &Path, index: usize) -> Result<(), git2::Error> {
    Repository::open(repo_path)?.stash_drop(index)
}

fn find_submodule<'a>(repo: &'a Repository, key: &str) -> Result<Submodule<'a>, git2::Error> {
    if let Ok(sm) = repo.find_submodule(key) {
        return Ok(sm);
    }
    if let Ok(subs) = repo.submodules() {
        for sm in subs {
            if sm.path().to_string_lossy() == key {
                return repo.find_submodule(sm.name().unwrap_or(key));
            }
        }
    }
    repo.find_submodule(key)
}

fn submodule_update_options<'a, F: FnMut(usize, usize) + 'a>(
    dir: PathBuf,
    mut progress: F,
) -> SubmoduleUpdateOptions<'a> {
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(credentials_cb(dir));
    cbs.transfer_progress(move |stats| {
        progress(stats.received_objects(), stats.total_objects());
        true
    });
    let mut fetch = FetchOptions::new();
    fetch.remote_callbacks(cbs);
    let mut opts = SubmoduleUpdateOptions::new();
    opts.fetch(fetch);
    opts
}

pub fn submodule_init<F: FnMut(usize, usize)>(
    parent_path: &Path,
    key: &str,
    progress: F,
) -> Result<(), git2::Error> {
    let repo = Repository::open(parent_path)?;
    let mut sm = find_submodule(&repo, key)?;
    sm.init(false)?;
    let mut opts = submodule_update_options(credential_dir(&repo), progress);
    sm.update(true, Some(&mut opts))
}

pub fn submodule_update<F: FnMut(usize, usize)>(
    parent_path: &Path,
    key: &str,
    progress: F,
) -> Result<(), git2::Error> {
    let repo = Repository::open(parent_path)?;
    let mut sm = find_submodule(&repo, key)?;
    let mut opts = submodule_update_options(credential_dir(&repo), progress);
    sm.update(true, Some(&mut opts))
}

pub fn stage_submodule_pointer(parent_path: &Path, key: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(parent_path)?;
    let sm = find_submodule(&repo, key)?;
    let path = sm.path().to_string_lossy().into_owned();
    let mut index = repo.index()?;
    index.add_all([path].iter(), IndexAddOption::DEFAULT, None)?;
    index.write()
}

fn drive_rebase(repo: &Repository, rebase: &mut Rebase) -> Result<SeqOutcome, git2::Error> {
    let sig = gitconfig::signature(repo)?;
    while let Some(op) = rebase.next() {
        op?;
        let index = repo.index()?;
        if index.has_conflicts() {
            return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
        }
        commit_op(rebase, &sig)?;
    }
    rebase.finish(Some(&sig))?;
    Ok(SeqOutcome::Done)
}

fn commit_op(rebase: &mut Rebase, sig: &Signature) -> Result<(), git2::Error> {
    match rebase.commit(None, sig, None) {
        Ok(_) => Ok(()),
        Err(e) if e.code() == ErrorCode::Applied => Ok(()),
        Err(e) => Err(e),
    }
}

pub(crate) fn conflict_paths(index: &Index) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(conflicts) = index.conflicts() {
        for c in conflicts.flatten() {
            if let Some(entry) = c.our.or(c.their).or(c.ancestor)
                && let Ok(p) = std::str::from_utf8(&entry.path)
            {
                let p = p.to_string();
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub fn commit(repo_path: &Path, message: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let sig = gitconfig::signature(&repo)?;

    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    let parents = match repo.head() {
        Ok(head) => vec![head.peel_to_commit()?],
        Err(_) => Vec::new(),
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)?;
    Ok(())
}

pub fn amend(repo_path: &Path, message: Option<&str>) -> Result<Oid, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.head()?.peel_to_commit()?;
    let sig = gitconfig::signature(&repo)?;
    let mut index = repo.index()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    commit.amend(Some("HEAD"), None, Some(&sig), None, message, Some(&tree))
}

pub fn head_message(repo_path: &Path) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    commit.message().ok().map(String::from)
}

pub fn head_has_commit(repo_path: &Path) -> bool {
    let Ok(repo) = Repository::open(repo_path) else {
        return false;
    };
    repo.head().and_then(|h| h.peel_to_commit()).is_ok()
}

pub fn head_is_pushed(repo_path: &Path) -> bool {
    let Ok(repo) = Repository::open(repo_path) else {
        return false;
    };
    let Ok(head) = repo.head() else {
        return false;
    };
    if !head.is_branch() {
        return false;
    }
    let Ok(name) = head.shorthand() else {
        return false;
    };
    let Ok(branch) = repo.find_branch(name, git2::BranchType::Local) else {
        return false;
    };
    match branch.upstream() {
        Ok(upstream) => upstream.get().target() == head.target(),
        Err(_) => false,
    }
}

pub struct RemoteInfo {
    pub name: String,
    pub url: Option<String>,
}

pub fn remotes(repo_path: &Path) -> Vec<RemoteInfo> {
    let mut out = Vec::new();
    if let Ok(repo) = Repository::open(repo_path)
        && let Ok(names) = repo.remotes()
    {
        for name in names.iter().flatten().flatten() {
            let url = repo
                .find_remote(name)
                .ok()
                .and_then(|r| r.url().ok().map(String::from));
            out.push(RemoteInfo {
                name: name.to_string(),
                url,
            });
        }
    }
    out
}

pub fn primary_remote(repo_path: &Path) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    if let Ok(head) = repo.head()
        && let Ok(branch) = head.shorthand()
        && let Ok(cfg) = repo.config()
        && let Ok(remote) = cfg.get_string(&format!("branch.{branch}.remote"))
    {
        return Some(remote);
    }
    let names = repo.remotes().ok()?;
    if names.iter().flatten().flatten().any(|n| n == "origin") {
        return Some("origin".to_string());
    }
    names.get(0).ok().flatten().map(String::from)
}

pub fn head_push_refspec(repo_path: &Path) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    let name = head.shorthand().ok()?;
    Some(format!("refs/heads/{name}:refs/heads/{name}"))
}

fn credential_dir(repo: &Repository) -> PathBuf {
    repo.workdir().unwrap_or(repo.path()).to_path_buf()
}

fn helper_cred(dir: &Path, url: &str, username: Option<&str>) -> Option<(String, String)> {
    let mut input = format!("url={url}\n");
    if let Some(user) = username {
        input.push_str(&format!("username={user}\n"));
    }
    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::to_string)
    };
    Some((field("username=")?, field("password=")?))
}

fn credentials_cb(
    dir: PathBuf,
) -> impl FnMut(&str, Option<&str>, CredentialType) -> Result<Cred, git2::Error> {
    move |url, username, allowed| {
        if allowed.contains(CredentialType::USERNAME) {
            return Cred::username(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::SSH_KEY) {
            return Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let Some((user, pass)) = helper_cred(&dir, url, username) {
                return Cred::userpass_plaintext(&user, &pass);
            }
            return Err(git2::Error::from_str(&format!(
                "no credential helper returned a username and password for {url}; check \
                 `git config --get-all credential.helper` (and `gh auth status` when the \
                 GitHub CLI helper is configured)"
            )));
        }
        if allowed.contains(CredentialType::DEFAULT) {
            return Cred::default();
        }
        Err(git2::Error::from_str(
            "no supported authentication method available",
        ))
    }
}

pub fn fetch<F: FnMut(usize, usize)>(
    repo_path: &Path,
    remote: &str,
    mut progress: F,
) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(credentials_cb(credential_dir(&repo)));
    cbs.transfer_progress(|stats| {
        progress(stats.received_objects(), stats.total_objects());
        true
    });
    let mut opts = FetchOptions::new();
    opts.remote_callbacks(cbs);
    let mut remote = repo.find_remote(remote)?;
    remote.fetch(&[] as &[&str], Some(&mut opts), None)?;
    Ok(())
}

pub fn push<F: FnMut(usize, usize)>(
    repo_path: &Path,
    remote: &str,
    refspecs: &[String],
    mut progress: F,
) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let reject: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(credentials_cb(credential_dir(&repo)));
    cbs.push_transfer_progress(|current, total, _bytes| progress(current, total));
    cbs.push_update_reference(|refname, status| {
        if let Some(msg) = status {
            *reject.borrow_mut() = Some(format!("{refname}: {msg}"));
        }
        Ok(())
    });
    let mut opts = PushOptions::new();
    opts.remote_callbacks(cbs);
    let mut remote = repo.find_remote(remote)?;
    remote.push(refspecs, Some(&mut opts))?;
    drop(opts);
    match reject.into_inner() {
        Some(msg) => Err(git2::Error::from_str(&msg)),
        None => Ok(()),
    }
}

pub fn delete_remote_branch<F: FnMut(usize, usize)>(
    repo_path: &Path,
    remote: &str,
    branch: &str,
    progress: F,
) -> Result<(), git2::Error> {
    let refspec = format!(":refs/heads/{branch}");
    push(repo_path, remote, std::slice::from_ref(&refspec), progress)?;

    let repo = Repository::open(repo_path)?;
    if let Ok(mut tracking) = repo.find_reference(&format!("refs/remotes/{remote}/{branch}")) {
        tracking.delete()?;
    }
    Ok(())
}

pub fn pull<F: FnMut(usize, usize)>(
    repo_path: &Path,
    remote_name: &str,
    mut progress: F,
) -> Result<SeqOutcome, git2::Error> {
    let repo = Repository::open(repo_path)?;
    {
        let mut cbs = RemoteCallbacks::new();
        cbs.credentials(credentials_cb(credential_dir(&repo)));
        cbs.transfer_progress(|stats| {
            progress(stats.received_objects(), stats.total_objects());
            true
        });
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(cbs);
        let mut remote = repo.find_remote(remote_name)?;
        remote.fetch(&[] as &[&str], Some(&mut opts), None)?;
    }

    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;
    let analysis = repo.merge_analysis(&[&fetch_commit])?.0;

    if analysis.is_up_to_date() {
        return Ok(SeqOutcome::Done);
    }

    if analysis.is_fast_forward() {
        let obj = repo.find_object(fetch_commit.id(), None)?;
        repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))?;
        let refname = repo.head()?.name().map(String::from)?;
        repo.find_reference(&refname)?
            .set_target(fetch_commit.id(), "pull: fast-forward")?;
        return Ok(SeqOutcome::Done);
    }

    repo.merge(&[&fetch_commit], None, None)?;
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(SeqOutcome::Conflicts(conflict_paths(&index)));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head()?.peel_to_commit()?;
    let their = repo.find_commit(fetch_commit.id())?;
    let sig = gitconfig::signature(&repo)?;
    let msg = repo
        .message()
        .unwrap_or_else(|_| format!("Merge commit '{}'", fetch_commit.id()));
    repo.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&head, &their])?;
    repo.cleanup_state()?;
    Ok(SeqOutcome::Done)
}

pub fn checkout_tracking(repo_path: &Path, remote_ref: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let local = remote_ref
        .split_once('/')
        .map(|(_, b)| b)
        .unwrap_or(remote_ref);

    let obj = repo.revparse_single(&format!("refs/remotes/{remote_ref}"))?;
    let commit = obj.peel_to_commit()?;

    if repo.find_branch(local, git2::BranchType::Local).is_err() {
        let mut branch = repo.branch(local, &commit, false)?;
        let _ = branch.set_upstream(Some(remote_ref));
    }

    let refname = format!("refs/heads/{local}");
    repo.checkout_tree(
        commit.as_object(),
        Some(git2::build::CheckoutBuilder::new().safe()),
    )?;
    repo.set_head(&refname)
}

#[cfg(test)]
mod tests {
    use super::helper_cred;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn credential_fill_runs_first_helper_with_hasconfig_include() {
        let tmp = std::env::temp_dir().join(format!("twit-cred-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        git(&tmp, &["init", "-q"]);
        git(
            &tmp,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/acme/x.git",
            ],
        );
        let account = tmp.join("account.gitconfig");
        std::fs::write(&account, "[test]\n\tpass = secret\n").unwrap();
        git(
            &tmp,
            &[
                "config",
                "includeIf.hasconfig:remote.*.url:**/acme/**.path",
                account.to_str().unwrap(),
            ],
        );
        for helper in [
            "",
            "!f() { echo username=u; echo password=$(git config --get test.pass); }; f",
            "!f() { echo username=wrong; echo password=wrong; }; f",
        ] {
            git(&tmp, &["config", "--add", "credential.helper", helper]);
        }
        let got = helper_cred(&tmp, "https://example.invalid/acme/x.git", None);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(got, Some(("u".to_string(), "secret".to_string())));
    }
}
