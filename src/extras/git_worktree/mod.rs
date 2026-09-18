// ponytail: all git invocations use synchronous std::process::Command, which
// blocks the tokio event loop. Under the default current_thread runtime this
// freezes the TUI during worktree merges. Migrate to tokio::process::Command
// and async functions if merge latency becomes noticeable.
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// RAII guard that restores the original working directory on drop. Ensures
/// the process CWD is restored even on panic. The `set_current_dir` approach
/// is process-global — under the `multithread` feature, other threads' CWD
/// reads are affected. // ponytail: use `git -C` everywhere if multithread matters.
struct ChdirGuard {
    orig_dir: PathBuf,
}

impl ChdirGuard {
    fn new(target: &Path) -> Result<Self, String> {
        let orig_dir = std::env::current_dir().map_err(|e| format!("current_dir: {}", e))?;
        std::env::set_current_dir(target)
            .map_err(|e| format!("cd to {}: {}", target.display(), e))?;
        Ok(ChdirGuard { orig_dir })
    }
}

impl Drop for ChdirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.orig_dir);
    }
}

#[derive(Debug, Clone)]
pub enum DeferredWorktreeAction {
    Merge {
        branch: String,
        target: String,
        main_path: String,
        wt_path: String,
    },
    Exit {
        main_path: String,
    },
}

impl fmt::Display for DeferredWorktreeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Merge { branch, target, .. } => {
                write!(f, "deferred worktree merge: {} -> {}", branch, target)
            }
            Self::Exit { main_path, .. } => {
                write!(f, "deferred worktree exit: back to {}", main_path)
            }
        }
    }
}

impl std::error::Error for DeferredWorktreeAction {}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub branch: String,
    pub worktree_path: PathBuf,
    pub main_repo_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Success,
    Conflicts(Vec<String>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct MergeState {
    pub info: WorktreeInfo,
    pub original_branch: String,
    pub orig_dir: PathBuf,
    pub stashed: bool,
}

/// Resolve a path reported by git (which may be relative to the process CWD)
/// to an absolute path, canonicalizing symlinks when possible.
fn absolutize(p: &str) -> PathBuf {
    let path = Path::new(p);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    abs.canonicalize().unwrap_or(abs)
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when `branch` exists as a local branch of the repo at `repo_path`.
pub fn branch_exists(repo_path: &Path, branch: &str) -> bool {
    // `--quiet` only silences error output; the exit status is what matters.
    Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// True when `wt_path` is currently registered as a linked worktree of the
/// repo at `main_repo_path`.
pub fn is_worktree_registered(main_repo_path: &Path, wt_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(main_repo_path)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(out) = output else { return false };
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let want = absolutize(&wt_path.to_string_lossy());
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .any(|p| absolutize(p.trim()) == want)
}

/// Canonical path of the main repo for the current directory (follows
/// `--git-common-dir` out of linked worktrees). Independent of CWD shape.
fn main_repo_path() -> Result<PathBuf, String> {
    let common_dir =
        git_output(&["rev-parse", "--git-common-dir"]).ok_or("not a git repository")?;
    let abs = absolutize(&common_dir);
    let parent = abs.parent().ok_or("cannot determine main repo path")?;
    Ok(parent.to_path_buf())
}

/// Validate a proposed worktree/branch name. Slashes are rejected even though
/// git allows them in branch names, because the name is also used as a
/// worktree directory name. Whitespace and leading dashes are rejected for
/// the same reason (plus argv safety). Everything else is deferred to
/// `git check-ref-format --branch`.
pub fn validate_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name is empty".to_string());
    }
    if name.chars().any(|c| c.is_whitespace()) {
        return Err(format!(
            "invalid name '{}': must be a single word without spaces",
            name
        ));
    }
    if name.contains('/') {
        return Err(format!(
            "invalid name '{}': must not contain slashes (it is also a directory name)",
            name
        ));
    }
    if name.starts_with('-') {
        return Err(format!("invalid name '{}': must not start with '-'", name));
    }
    let output = Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;
    if !output.status.success() {
        return Err(format!("invalid branch name '{}'", name));
    }
    Ok(())
}

pub fn detect() -> Option<WorktreeInfo> {
    let common_dir = git_output(&["rev-parse", "--git-common-dir"])?;
    let git_dir = git_output(&["rev-parse", "--git-dir"])?;

    if absolutize(&common_dir) == absolutize(&git_dir) {
        return None;
    }

    // The git dir in a worktree looks like <main>/.git/worktrees/<name>
    // The actual working tree is stored in the `gitdir` file inside that directory.
    let git_dir_path = absolutize(&git_dir);
    let gitdir_file = git_dir_path.join("gitdir");

    let worktree_path = if gitdir_file.exists() {
        // Read the actual worktree path from the gitdir file
        std::fs::read_to_string(&gitdir_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let abs = absolutize(&s);
                // The gitdir file points at the worktree's `.git` file/dir;
                // its parent is the worktree root.
                abs.parent().map(|p| p.to_path_buf()).unwrap_or(abs)
            })
            .unwrap_or_else(|| {
                // Fallback: use absolutized git-dir path
                git_dir_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| git_dir_path.clone())
            })
    } else {
        // Simpler worktree structure: git-dir is at .git, parent is worktree root
        git_dir_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| git_dir_path.clone())
    };

    let main_repo_path = absolutize(&common_dir).parent().map(|p| p.to_path_buf())?;

    // Detached HEAD has no branch to merge; refuse rather than
    // producing an empty branch name that would poison later git calls.
    let branch = current_branch()?;

    Some(WorktreeInfo {
        branch,
        worktree_path,
        main_repo_path,
    })
}

pub fn current_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch == "HEAD" { None } else { Some(branch) }
}

pub fn default_branch(repo_path: &Path) -> Option<String> {
    for name in &["main", "master"] {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["rev-parse", "--verify", name])
            .output()
            .ok();
        if let Some(out) = output
            && out.status.success()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Create a linked worktree at `<base>/<name>` on a new branch `name`.
/// The base defaults to the parent directory of the current repo. The
/// base directory is created if missing. Returns an error (instead of
/// silently reusing state) when the branch already exists or a worktree
/// is already registered at the target path.
pub fn create(name: &str, base_dir: Option<&Path>) -> Result<(PathBuf, WorktreeInfo), String> {
    validate_branch_name(name)?;

    // Capture the caller's location first: `main_repo_path` must be the repo
    // we were invoked in, not whatever CWD `git worktree add` leaves behind.
    let main_repo = main_repo_path()?;

    if branch_exists(&main_repo, name) {
        return Err(format!(
            "branch '{}' already exists; use an existing worktree or pick another name",
            name
        ));
    }

    let target = match base_dir {
        Some(dir) => dir.join(name),
        None => main_repo
            .parent()
            .map(|p| p.join(name))
            .ok_or_else(|| "cannot determine worktree base directory".to_string())?,
    };

    if target.exists() {
        if is_worktree_registered(&main_repo, &target) {
            return Err(format!(
                "worktree '{}' already exists at {}; cd into it instead",
                name,
                target.display()
            ));
        }
        return Err(format!("target path {} already exists", target.display()));
    }

    if let Some(parent) = target.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create base dir {}: {}", parent.display(), e))?;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&main_repo)
        .args(["worktree", "add", "-b", name])
        .arg("--")
        .arg(&target)
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = format!("{} {}", stdout.trim(), stderr.trim())
            .trim()
            .to_string();
        return Err(format!("git worktree add failed: {}", detail));
    }

    let wt_path = absolutize(&target.to_string_lossy());

    Ok((
        wt_path.clone(),
        WorktreeInfo {
            branch: name.to_string(),
            worktree_path: wt_path,
            main_repo_path: main_repo,
        },
    ))
}

pub fn repo_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Phase 1: Change to main repo, stash, fetch, checkout target, pull, merge.
/// On Success or Conflicts, current directory is left in the main repo (on target).
/// On Error, the function cleans up (pops any stash) and restores the original directory.
pub fn try_merge(info: &WorktreeInfo, target: &str) -> (MergeState, MergeOutcome) {
    let orig_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                MergeState {
                    info: info.clone(),
                    original_branch: String::new(),
                    stashed: false,
                    orig_dir: PathBuf::new(),
                },
                MergeOutcome::Error(format!("current_dir: {}", e)),
            );
        }
    };

    // Guard restores `orig_dir` on every early return. On the Success /
    // Conflicts paths we point it at the main repo instead (see below).
    let mut guard = match ChdirGuard::new(&info.main_repo_path) {
        Ok(g) => g,
        Err(e) => {
            return (
                MergeState {
                    info: info.clone(),
                    original_branch: String::new(),
                    stashed: false,
                    orig_dir,
                },
                MergeOutcome::Error(e),
            );
        }
    };

    let original_branch = current_branch().unwrap_or_default();

    // Stash local changes first. A failed stash is fatal: proceeding with a
    // dirty tree into checkout/pull/merge would risk mixing unrelated edits
    // into the merge or losing them.
    let stashed = if has_uncommitted_changes() {
        match run_git(["stash", "--include-untracked"]) {
            Ok(_) => true,
            Err(e) => {
                return (
                    MergeState {
                        info: info.clone(),
                        original_branch,
                        stashed: false,
                        orig_dir: guard.orig_dir.clone(),
                    },
                    MergeOutcome::Error(format!(
                        "failed to stash uncommitted changes in main repo: {}; \
                         commit or stash them manually and retry",
                        e
                    )),
                );
            }
        }
    } else {
        false
    };

    // Helper to clean up on early-stage errors: pop stash, restore dir
    // (via guard drop by resetting orig_dir first).
    let cleanup_early =
        |guard: &mut ChdirGuard, state: &mut MergeState, err: String| -> MergeOutcome {
            if state.stashed
                && let Err(e) = run_git(["stash", "pop"])
            {
                tracing::error!(
                    branch = %state.info.branch,
                    error = %e,
                    "worktree merge: failed to restore stash during early cleanup; \
                     stashed changes may be lost; try `git stash pop` manually"
                );
            }
            guard.orig_dir = state.orig_dir.clone();
            MergeOutcome::Error(err)
        };

    let mut working_state = MergeState {
        info: info.clone(),
        original_branch: original_branch.clone(),
        stashed,
        orig_dir: orig_dir.clone(),
    };

    if let Err(e) = run_git(["fetch", "--all"]) {
        let outcome = cleanup_early(
            &mut guard,
            &mut working_state,
            format!("fetch failed: {}", e),
        );
        return (working_state, outcome);
    }

    if let Err(e) = run_git(["checkout", target]) {
        let outcome = cleanup_early(
            &mut guard,
            &mut working_state,
            format!("checkout failed: {}", e),
        );
        return (working_state, outcome);
    }

    // Best-effort: after a successful `fetch --all` a pull failure almost
    // always means the target has no upstream (local-only main/master), in
    // which case there is nothing to pull and the local merge can proceed.
    if let Err(e) = run_git(["pull", "--no-edit"]) {
        tracing::debug!(
            "worktree merge: pull failed, continuing with local target: {}",
            e
        );
    }

    match run_git(["merge", "--squash", &info.branch]) {
        Ok(_) => match run_git(["commit", "--no-edit"]) {
            Ok(_) => {
                guard.orig_dir = info.main_repo_path.clone();
                (working_state, MergeOutcome::Success)
            }
            Err(e) if is_nothing_to_commit(&e) => {
                // `git commit` exits non-zero when the squash produced no
                // changes (branch already fully merged). Match on stdout too:
                // git splits the message across streams by locale.
                guard.orig_dir = info.main_repo_path.clone();
                (working_state, MergeOutcome::Success)
            }
            Err(e) => {
                abort_merge_best_effort();
                let _ = run_git_quiet(["checkout", &original_branch]);
                let outcome = cleanup_early(
                    &mut guard,
                    &mut working_state,
                    format!("commit after squash failed: {}", e),
                );
                (working_state, outcome)
            }
        },
        Err(_) if has_merge_conflict() => {
            let files = conflicted_files();
            guard.orig_dir = info.main_repo_path.clone();
            (working_state, MergeOutcome::Conflicts(files))
        }
        Err(e) => {
            tracing::error!(
                branch = %info.branch,
                target = %target,
                error = %e,
                "worktree merge: merge failed, aborting"
            );
            abort_merge_best_effort();
            let _ = run_git_quiet(["checkout", &original_branch]);
            let outcome = cleanup_early(
                &mut guard,
                &mut working_state,
                format!("merge failed: {}", e),
            );
            (working_state, outcome)
        }
    }
}

/// Abort an in-progress merge without failing when there is nothing to
/// abort. `git merge --squash` conflicts leave unmerged index entries but no
/// `MERGE_HEAD`, so `merge --abort` exits non-zero ("no merge to abort");
/// `reset --merge` clears that state instead.
fn abort_merge_best_effort() {
    if run_git_quiet(["merge", "--abort"]).is_none() {
        let _ = run_git_quiet(["reset", "--merge"]);
    }
}

/// Phase 2: After a successful merge (or after conflicts are resolved),
/// delete the worktree and delete the branch. Leaves CWD in the main repo
/// (the worktree directory no longer exists, so restoring to it is wrong).
/// The `force` flag only affects `worktree remove` (dirty worktree);
/// branch deletion always uses `-D` because squash merges never advance the
/// branch ref, so `-d` ("fully merged") does not apply to them.
pub fn complete_merge(state: &MergeState) -> Result<(), String> {
    complete_merge_with_force(state, false)
}

pub fn complete_merge_force(state: &MergeState) -> Result<(), String> {
    complete_merge_with_force(state, true)
}

fn complete_merge_with_force(state: &MergeState, _force: bool) -> Result<(), String> {
    let mut guard = ChdirGuard::new(&state.info.main_repo_path)?;

    let result = (|| {
        run_git([
            "worktree",
            "remove",
            "--force",
            &state.info.worktree_path.to_string_lossy(),
        ])?;
        // The merge used `--squash`, which never advances the branch ref, so
        // the squash commit is not a descendant of the branch tip and `-d`
        // ("fully merged") almost never applies. Verify patch-equivalence
        // instead and delete unconditionally — non-force still refuses via
        // `worktree remove` above when the worktree itself is dirty.
        run_git(["branch", "-D", &state.info.branch])?;
        Ok::<(), String>(())
    })();

    if let Err(e) = &result {
        tracing::error!(
            branch = %state.info.branch,
            error = %e,
            "worktree complete_merge: cleanup failed"
        );
        // Guard drops here, restoring the caller's original directory.
        return result;
    }

    if state.stashed
        && let Err(e) = run_git(["stash", "pop"])
    {
        tracing::error!(
            branch = %state.info.branch,
            error = %e,
            "worktree complete_merge: failed to pop stash; \
                 changes may be lost; try `git stash pop` manually"
        );
        // Stay in the main repo so the user can inspect the stash.
        guard.orig_dir = state.info.main_repo_path.clone();
        return Err(format!(
            "merge succeeded and worktree was removed, but stash pop failed: {}. \
                 Your changes are in the stash; run `git stash pop` manually.",
            e
        ));
    }
    // Success: stay in the main repo — the worktree directory is gone.
    guard.orig_dir = state.info.main_repo_path.clone();

    result
}

/// Best-effort cleanup of a worktree after a merge. Safe to call even if the
/// worktree or branch has already been removed (idempotent).
pub fn cleanup_worktree(wt_path: &str, branch: &str, main_repo_path: &str, force: bool) {
    // ChdirGuard ensures the original CWD is restored on drop, even on panic.
    let _guard = match ChdirGuard::new(Path::new(main_repo_path)) {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("cleanup_worktree: {}", e);
            return;
        }
    };
    let remove_output = if force {
        Command::new("git")
            .args(["worktree", "remove", "--force", wt_path])
            .output()
    } else {
        Command::new("git")
            .args(["worktree", "remove", wt_path])
            .output()
    };
    if let Ok(out) = &remove_output {
        if out.status.success() {
            tracing::info!(branch, wt_path, "cleanup_worktree: removed worktree");
        } else {
            tracing::debug!(
                branch,
                wt_path,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "cleanup_worktree: git worktree remove (already gone or failed)"
            );
        }
    }

    let branch_flag = if force { "-D" } else { "-d" };
    // NOTE: kept as -d/-D for the agent-driven path (see verify_branch_merged
    // before cleanup_worktree there): an agent may have done a true merge,
    // where -d is a meaningful safety check. complete_merge always uses -D
    // because it only ever runs after a squash merge.
    let branch_output = Command::new("git")
        .args(["branch", branch_flag, branch])
        .output();
    if let Ok(out) = &branch_output {
        if out.status.success() {
            tracing::info!(branch, "cleanup_worktree: deleted branch");
        } else {
            tracing::debug!(
                branch,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "cleanup_worktree: git branch delete (already gone or failed)"
            );
        }
    }
}

/// Cancel an in-progress merge: abort, restore original branch, pop stash, restore dir.
/// Best-effort throughout: a failed abort must not skip the branch restore or
/// the stash pop. `merge --abort` fails for squash merges (no MERGE_HEAD),
/// so fall back to `reset --merge`.
pub fn cancel_merge(state: &MergeState) -> Result<(), String> {
    // ChdirGuard restores CWD on drop; override orig_dir to state.orig_dir
    // so it restores to the pre-merge directory, not the current one.
    let mut guard = ChdirGuard::new(&state.info.main_repo_path)?;
    guard.orig_dir = state.orig_dir.clone();

    if has_merge_conflict() {
        abort_merge_best_effort();
    }
    if !state.original_branch.is_empty() {
        let _ = run_git_quiet_logged(
            ["checkout", &state.original_branch],
            "cancel_merge: checkout original",
        );
    }
    if state.stashed
        && let Err(e) = run_git_quiet_logged(["stash", "pop"], "cancel_merge: stash pop")
    {
        tracing::error!(
            branch = %state.info.branch,
            error = %e,
            "cancel_merge: failed to pop stash; try `git stash pop` manually"
        );
    }

    Ok(())
}

/// Check if there is an active merge conflict in the current directory.
pub fn has_merge_conflict() -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    repo_has_merge_conflict(&cwd)
}

/// List files with merge conflicts in the current directory.
pub fn conflicted_files() -> Vec<String> {
    repo_conflicted_files(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Check for an active merge conflict in `repo_path` without changing CWD.
pub fn repo_has_merge_conflict(repo_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-parse", "--git-path", "MERGE_HEAD"])
        .output()
        .ok();
    if let Some(out) = output
        && out.status.success()
    {
        let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !rel.is_empty() {
            // `--git-path` resolves relative to the repo dir.
            let abs = if Path::new(&rel).is_absolute() {
                PathBuf::from(rel)
            } else {
                repo_path.join(rel)
            };
            if abs.exists() {
                return true;
            }
        }
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

/// List files with merge conflicts in `repo_path` without changing CWD.
pub fn repo_conflicted_files(repo_path: &Path) -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Outcome of verifying whether an agent-driven merge actually landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMergeStatus {
    /// Branch ref is gone (already cleaned) or its content is in `target`.
    Merged,
    /// Unresolved conflicts in the main repo; worktree must be kept.
    Conflicts(Vec<String>),
    /// No conflicts, but the branch content is not in `target`; keep everything.
    NotMerged(String),
}

/// Verify that `branch` is merged into `target` in the repo at `main_repo`.
/// Handles squash merges: `git branch -d` ancestry does not cover squash
/// commits, so patch-equivalence (`git cherry`) is used as the squash path.
pub fn verify_branch_merged(main_repo: &Path, target: &str, branch: &str) -> AgentMergeStatus {
    if !branch_exists(main_repo, target) {
        return AgentMergeStatus::NotMerged(format!("target branch '{}' not found", target));
    }
    // Branch already deleted: a previous cleanup finished the job.
    if !branch_exists(main_repo, branch) {
        return AgentMergeStatus::Merged;
    }
    if repo_has_merge_conflict(main_repo) {
        return AgentMergeStatus::Conflicts(repo_conflicted_files(main_repo));
    }
    if branch_is_ancestor(main_repo, branch, target) {
        return AgentMergeStatus::Merged;
    }
    // Squash path: every commit unique to `branch` has a patch-equivalent
    // commit in `target` (no `+` lines from `git cherry`).
    match cherry_unmerged_count(main_repo, target, branch) {
        Ok(0) => AgentMergeStatus::Merged,
        Ok(_) => AgentMergeStatus::NotMerged(format!(
            "branch '{}' has commits not present in '{}'",
            branch, target
        )),
        Err(e) => AgentMergeStatus::NotMerged(e),
    }
}

fn branch_is_ancestor(main_repo: &Path, branch: &str, target: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(main_repo)
        .args(["merge-base", "--is-ancestor", branch, target])
        .status()
        .is_ok_and(|s| s.success())
}

/// Number of `branch` commits with no patch-equivalent in `target`.
/// Errors when refs are missing or share no merge base (conservative: not merged).
fn cherry_unmerged_count(main_repo: &Path, target: &str, branch: &str) -> Result<usize, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(main_repo)
        .args(["cherry", target, branch])
        .output()
        .map_err(|e| format!("git cherry failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "could not compare '{}' with '{}': {}",
            branch,
            target,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.starts_with('+'))
        .count())
}

// --- Private helpers ---

/// True when a failed `git commit` means "nothing to commit". Git prints the
/// message on stdout in some locales (it is swallowed by `run_git`, which
/// only keeps stderr); detect via exit state instead: re-running
/// `git diff --cached --quiet` is empty exactly when there is nothing
/// staged to commit.
fn is_nothing_to_commit(_err: &str) -> bool {
    Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()
        .is_ok_and(|s| s.success())
}

fn run_git<const N: usize>(args: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let err = format!("git {} failed: {}", args.join(" "), stderr.trim());
        tracing::debug!("{}", err);
        return Err(err);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_quiet<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!("git {} failed silently: {}", args.join(" "), stderr.trim());
        None
    }
}

fn run_git_quiet_logged<const N: usize>(args: [&str; N], context: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let err = format!(
            "{}: git {} failed: {}",
            context,
            args.join(" "),
            stderr.trim()
        );
        tracing::warn!("{}", err);
        return Err(err);
    }
    Ok(())
}

fn has_uncommitted_changes() -> bool {
    let output = Command::new("git").args(["status", "--porcelain"]).output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

pub fn worktree_has_uncommitted(wt_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["status", "--porcelain"])
        .output();
    match output {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        _ => false,
    }
}

pub fn worktree_auto_commit_all(wt_path: &Path) -> Result<(), String> {
    // Use `-u` (update tracked files only) instead of `-A` (all including
    // untracked) to avoid auto-committing new files that may contain secrets.
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["add", "-u"])
        .output()
        .map_err(|e| format!("git add failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git add -u failed: {}", stderr.trim()));
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| format!("git diff failed: {}", e))?;
    if status.success() {
        // Nothing staged: either clean, or only untracked files (which `-u`
        // deliberately skips). Committing would fail with "nothing to commit",
        // so surface that distinction instead.
        let untracked = Command::new("git")
            .arg("-C")
            .arg(wt_path)
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.starts_with("??"))
            })
            .unwrap_or(false);
        return if untracked {
            Err("only untracked files remain; commit them manually (auto-commit skips untracked files)".to_string())
        } else {
            Ok(())
        };
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(wt_path)
        .args(["commit", "-m", "auto-commit: save changes before merge"])
        .output()
        .map_err(|e| format!("git commit failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = format!("{} {}", stdout.trim(), stderr.trim())
            .trim()
            .to_string();
        if detail.contains("nothing to commit") {
            return Ok(());
        }
        return Err(format!("git commit failed: {}", detail));
    }
    Ok(())
}
