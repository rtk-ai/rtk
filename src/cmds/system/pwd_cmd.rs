//! Reports execution context: physical cwd, git root/worktree linkage, branch.
//!
//! Agent harnesses (Claude Code, Cursor, ...) track their shell cwd
//! out-of-band and can silently drift to the wrong checkout when git
//! worktrees are involved (see issue #2148: commands believed to run in a
//! linked worktree actually ran in the main checkout). `rtk pwd` prints the
//! physical cwd plus the git context in one compact block so an agent (or a
//! hook) can validate where commands will actually execute before writing.
//!
//! Output examples:
//! ```text
//! /repo/.worktrees/feature
//! branch: feature
//! worktree of: /repo (branch develop)
//! ```
//!
//! A shell's `pwd` builtin prints the logical, symlink-preserving `$PWD`;
//! child processes only ever see the physical `getcwd()`. When the two
//! disagree, a `note:`/`warn:` line makes the discrepancy explicit instead
//! of leaving the agent to misdiagnose it as misrouting.

use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let physical = std::env::current_dir().context("Failed to read current directory")?;
    if verbose > 0 {
        eprintln!("Resolving git context for {}", physical.display());
    }

    let logical = std::env::var_os("PWD").map(PathBuf::from);
    let output = describe(&physical, logical.as_deref());
    print!("{}", output);

    timer.track("pwd", "rtk pwd", &output, &output);
    Ok(())
}

/// What the enclosing `.git` entry says about this directory.
enum GitContext {
    /// `.git` is a directory — a main checkout.
    Main { root: PathBuf, branch: String },
    /// `.git` is a file pointing into `<shared>/worktrees/<name>` — a linked worktree.
    Worktree {
        root: PathBuf,
        branch: String,
        main_root: Option<PathBuf>,
        main_branch: Option<String>,
        /// Set when the shared gitdir itself belongs to a submodule
        /// (a linked worktree of a submodule checkout).
        submodule: Option<SubmoduleLink>,
    },
    /// `.git` is a file pointing into `<super>/.git/modules/<name>` — a submodule.
    Submodule {
        root: PathBuf,
        branch: String,
        link: SubmoduleLink,
    },
}

/// Link from a submodule gitdir back to the checkout that contains it.
struct SubmoduleLink {
    /// The top-level superproject root (the checkout whose `.git/modules/`
    /// holds the gitdir). For nested submodules this is the outermost
    /// superproject, not the immediate parent submodule — `nested` flags that.
    superproject: PathBuf,
    nested: bool,
}

impl SubmoduleLink {
    fn describe(&self) -> String {
        if self.nested {
            format!("submodule (nested) of: {}\n", self.superproject.display())
        } else {
            format!("submodule of: {}\n", self.superproject.display())
        }
    }
}

fn describe(physical: &Path, logical: Option<&Path>) -> String {
    let mut out = format!("{}\n", physical.display());

    match find_git_context(physical) {
        Some(GitContext::Main { root, branch }) => {
            out.push_str(&format!("branch: {}\n", branch));
            if root != physical {
                out.push_str(&format!("repo root: {}\n", root.display()));
            }
        }
        Some(GitContext::Worktree {
            root,
            branch,
            main_root,
            main_branch,
            submodule,
        }) => {
            out.push_str(&format!("branch: {}\n", branch));
            if root != physical {
                out.push_str(&format!("worktree root: {}\n", root.display()));
            }
            match (main_root, main_branch) {
                (Some(main), Some(b)) => out.push_str(&format!(
                    "worktree of: {} (branch {})\n",
                    main.display(),
                    b
                )),
                (Some(main), None) => {
                    out.push_str(&format!("worktree of: {}\n", main.display()))
                }
                (None, _) => out.push_str("worktree of: (unresolved gitdir pointer)\n"),
            }
            if let Some(link) = submodule {
                out.push_str(&link.describe());
            }
        }
        Some(GitContext::Submodule { root, branch, link }) => {
            out.push_str(&format!("branch: {}\n", branch));
            if root != physical {
                out.push_str(&format!("submodule root: {}\n", root.display()));
            }
            out.push_str(&link.describe());
        }
        None => out.push_str("(not in a git repository)\n"),
    }

    if let Some(note) = logical.and_then(|l| pwd_mismatch_note(physical, l)) {
        out.push_str(&note);
        out.push('\n');
    }
    out
}

/// Note when the shell's logical `$PWD` disagrees with the physical cwd.
///
/// Resolving to the same directory (symlinked path) is benign but worth
/// surfacing; resolving elsewhere means the shell and this process genuinely
/// disagree about where they are.
fn pwd_mismatch_note(physical: &Path, logical: &Path) -> Option<String> {
    if logical == physical {
        return None;
    }
    match fs::canonicalize(logical) {
        Ok(resolved) if resolved == physical => Some(format!(
            "note: shell $PWD is a symlinked path to the same directory: {}",
            logical.display()
        )),
        _ => Some(format!(
            "warn: shell $PWD differs from physical cwd: {}",
            logical.display()
        )),
    }
}

/// Walk up from `start` until a `.git` entry is found.
fn find_git_context(start: &Path) -> Option<GitContext> {
    let mut dir = start;
    loop {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(GitContext::Main {
                root: dir.to_path_buf(),
                branch: read_branch(&dot_git),
            });
        }
        if dot_git.is_file() {
            return Some(gitfile_context(dir, &dot_git));
        }
        dir = dir.parent()?;
    }
}

/// Classify a `.git` *file* (linked worktree or submodule pointer).
///
/// Worktree layout is checked before submodule layout: a linked worktree of
/// a submodule (gitdir `<super>/.git/modules/<name>/worktrees/<wt>`) must
/// keep its worktree linkage — that is the drift signal this command exists
/// to surface — and additionally reports the submodule attribution.
fn gitfile_context(root: &Path, dot_git_file: &Path) -> GitContext {
    let gitdir = match read_gitdir_pointer(dot_git_file, root) {
        Some(gitdir) => gitdir,
        None => {
            return GitContext::Worktree {
                root: root.to_path_buf(),
                branch: "(unknown)".to_string(),
                main_root: None,
                main_branch: None,
                submodule: None,
            }
        }
    };
    let branch = read_branch(&gitdir);

    // Real linked worktrees always carry a `commondir` file. The name-based
    // clause only covers degraded gitdirs, so exclude submodule-layout paths
    // there: a submodule at path `worktrees/<name>` has a gitdir whose parent
    // is also named `worktrees` and must not gain a bogus worktree link.
    let is_worktree_layout = gitdir.join("commondir").is_file()
        || (gitdir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "worktrees")
            && submodule_link(&gitdir).is_none());
    if is_worktree_layout {
        let common = common_git_dir(&gitdir);
        let (main_root, main_branch) = match common.as_deref() {
            // Shared gitdir is a checkout's `.git` — the normal case.
            Some(c) if c.file_name().is_some_and(|name| name == ".git") => {
                (c.parent().map(Path::to_path_buf), read_branch_opt(c))
            }
            // Shared gitdir is not `<checkout>/.git` (e.g. a submodule's
            // gitdir under `.git/modules/`): point at the gitdir itself
            // rather than fabricating a checkout path from its parent.
            Some(c) => (Some(c.to_path_buf()), read_branch_opt(c)),
            None => (None, None),
        };
        return GitContext::Worktree {
            root: root.to_path_buf(),
            branch,
            main_root,
            main_branch,
            submodule: common.as_deref().and_then(submodule_link),
        };
    }

    if let Some(link) = submodule_link(&gitdir) {
        return GitContext::Submodule {
            root: root.to_path_buf(),
            branch,
            link,
        };
    }

    GitContext::Worktree {
        root: root.to_path_buf(),
        branch,
        main_root: None,
        main_branch: None,
        submodule: None,
    }
}

/// Submodule gitdirs live under `<super>/.git/modules/...`; returns the link
/// when `gitdir` matches that layout. Attribution is always to the outermost
/// superproject (the checkout owning the `.git/modules/` tree); a further
/// `modules` component below it marks the submodule as nested.
///
/// The `nested` flag is best-effort: on disk, a nested submodule `a` → `b`
/// (`modules/a/modules/b`) is indistinguishable from a plain submodule whose
/// own path is `a/modules/b`, so the label can be a false positive for
/// submodule paths that contain a `modules` component.
fn submodule_link(gitdir: &Path) -> Option<SubmoduleLink> {
    gitdir.ancestors().find_map(|anc| {
        let parent = anc.parent()?;
        if anc.file_name()? == "modules" && parent.file_name()? == ".git" {
            let nested = gitdir
                .strip_prefix(anc)
                .ok()?
                .components()
                .skip(1) // first component is this submodule's own name
                .any(|c| c.as_os_str() == "modules");
            Some(SubmoduleLink {
                superproject: parent.parent()?.to_path_buf(),
                nested,
            })
        } else {
            None
        }
    })
}

/// A worktree's `.git` file contains `gitdir: <path/to/main/.git/worktrees/name>`.
fn read_gitdir_pointer(dot_git_file: &Path, base: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git_file).ok()?;
    let raw = content.strip_prefix("gitdir:")?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    let abs = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    Some(fs::canonicalize(&abs).unwrap_or(abs))
}

/// The shared `.git` dir of the main checkout: `<gitdir>/commondir` points at
/// it (usually `../..`); fall back to stripping `worktrees/<name>` from the path.
fn common_git_dir(gitdir: &Path) -> Option<PathBuf> {
    if let Ok(raw) = fs::read_to_string(gitdir.join("commondir")) {
        let p = PathBuf::from(raw.trim());
        let abs = if p.is_absolute() { p } else { gitdir.join(p) };
        return Some(fs::canonicalize(&abs).unwrap_or(abs));
    }
    let parent = gitdir.parent()?;
    if parent.file_name()? == "worktrees" {
        parent.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// Branch name from `<gitdir>/HEAD`: a `refs/heads/` branch, another symbolic
/// ref verbatim, or a short hash for detached HEAD. `None` when HEAD is
/// missing or unparseable.
fn read_branch_opt(gitdir: &Path) -> Option<String> {
    let head = fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }
    if let Some(other_ref) = head.strip_prefix("ref: ") {
        return Some(other_ref.to_string());
    }
    // Detached HEAD: only trust plausible object hashes (sha1 or sha256).
    if (7..=64).contains(&head.len()) && head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!(
            "detached @ {}",
            head.chars().take(9).collect::<String>()
        ));
    }
    None
}

fn read_branch(gitdir: &Path) -> String {
    read_branch_opt(gitdir).unwrap_or_else(|| "(unknown)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Canonicalized tempdir (macOS `/var` is a symlink to `/private/var`).
    fn tmpdir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create tempdir");
        let path = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        (dir, path)
    }

    /// Build a fake main checkout: `<root>/.git/` with HEAD on `branch`.
    fn make_main_checkout(root: &Path, branch: &str) {
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("create .git");
        fs::write(git.join("HEAD"), format!("ref: refs/heads/{}\n", branch)).expect("write HEAD");
    }

    /// Build a fake linked worktree of `main_root` at `wt_root` on `branch`.
    fn make_worktree(main_root: &Path, wt_root: &Path, name: &str, branch: &str) {
        let wt_gitdir = main_root.join(".git").join("worktrees").join(name);
        fs::create_dir_all(&wt_gitdir).expect("create worktree gitdir");
        fs::write(
            wt_gitdir.join("HEAD"),
            format!("ref: refs/heads/{}\n", branch),
        )
        .expect("write worktree HEAD");
        fs::write(wt_gitdir.join("commondir"), "../..\n").expect("write commondir");

        fs::create_dir_all(wt_root).expect("create worktree root");
        fs::write(
            wt_root.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .expect("write .git pointer file");
    }

    #[test]
    fn test_main_checkout() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        fs::create_dir_all(&root).expect("mkdir repo");
        make_main_checkout(&root, "develop");

        let out = describe(&root, None);
        assert_eq!(
            out,
            format!("{}\nbranch: develop\n", root.display()),
            "main checkout at root: cwd + branch only"
        );
    }

    #[test]
    fn test_subdirectory_shows_repo_root() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let sub = root.join("src").join("cmds");
        fs::create_dir_all(&sub).expect("mkdir subdir");
        make_main_checkout(&root, "main");

        let out = describe(&sub, None);
        assert!(out.contains("branch: main\n"));
        assert!(
            out.contains(&format!("repo root: {}\n", root.display())),
            "subdir must surface the repo root: {}",
            out
        );
    }

    #[test]
    fn test_linked_worktree() {
        let (_guard, tmp) = tmpdir();
        let main = tmp.join("main-checkout");
        let wt = tmp.join("wt");
        fs::create_dir_all(&main).expect("mkdir main");
        make_main_checkout(&main, "develop");
        make_worktree(&main, &wt, "wt", "feature");

        let out = describe(&wt, None);
        assert!(out.contains("branch: feature\n"), "worktree branch: {}", out);
        assert!(
            out.contains(&format!("worktree of: {} (branch develop)\n", main.display())),
            "worktree must link back to main checkout + its branch: {}",
            out
        );
    }

    #[test]
    fn test_worktree_without_commondir_falls_back() {
        let (_guard, tmp) = tmpdir();
        let main = tmp.join("main-checkout");
        let wt = tmp.join("wt");
        fs::create_dir_all(&main).expect("mkdir main");
        make_main_checkout(&main, "develop");
        make_worktree(&main, &wt, "wt", "feature");
        fs::remove_file(main.join(".git/worktrees/wt/commondir")).expect("drop commondir");

        let out = describe(&wt, None);
        assert!(
            out.contains(&format!("worktree of: {}", main.display())),
            "path-based fallback must still resolve the main checkout: {}",
            out
        );
    }

    #[test]
    fn test_not_a_repository() {
        let (_guard, tmp) = tmpdir();
        let dir = tmp.join("plain");
        fs::create_dir_all(&dir).expect("mkdir plain");

        let out = describe(&dir, None);
        assert!(out.contains("(not in a git repository)"), "{}", out);
    }

    #[test]
    fn test_detached_head() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("mkdir .git");
        fs::write(git.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n")
            .expect("write detached HEAD");

        let out = describe(&root, None);
        assert!(out.contains("branch: detached @ 012345678\n"), "{}", out);
    }

    #[test]
    fn test_pwd_note_symlinked_same_dir() {
        let (_guard, tmp) = tmpdir();
        let real = tmp.join("real");
        let link = tmp.join("link");
        fs::create_dir_all(&real).expect("mkdir real");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(not(unix))]
        return;

        let note = pwd_mismatch_note(&real, &link).expect("mismatch note expected");
        assert!(note.starts_with("note: shell $PWD is a symlinked path"), "{}", note);
    }

    #[test]
    fn test_pwd_warn_on_genuinely_different_dir() {
        let (_guard, tmp) = tmpdir();
        let a = tmp.join("a");
        let b = tmp.join("b");
        fs::create_dir_all(&a).expect("mkdir a");
        fs::create_dir_all(&b).expect("mkdir b");

        let note = pwd_mismatch_note(&a, &b).expect("mismatch note expected");
        assert!(note.starts_with("warn: shell $PWD differs"), "{}", note);
    }

    #[test]
    fn test_pwd_note_absent_when_equal() {
        let (_guard, tmp) = tmpdir();
        assert!(pwd_mismatch_note(&tmp, &tmp).is_none());
    }

    #[test]
    fn test_submodule_not_mislabeled_as_worktree() {
        let (_guard, tmp) = tmpdir();
        let superproject = tmp.join("super");
        let sub = superproject.join("libs").join("mylib");
        fs::create_dir_all(&sub).expect("mkdir submodule");
        make_main_checkout(&superproject, "main");

        let sub_gitdir = superproject.join(".git").join("modules").join("mylib");
        fs::create_dir_all(&sub_gitdir).expect("mkdir submodule gitdir");
        fs::write(sub_gitdir.join("HEAD"), "ref: refs/heads/main\n").expect("write sub HEAD");
        fs::write(
            sub.join(".git"),
            format!("gitdir: {}\n", sub_gitdir.display()),
        )
        .expect("write .git pointer file");

        let out = describe(&sub, None);
        assert!(out.contains("branch: main\n"), "{}", out);
        assert!(
            out.contains(&format!("submodule of: {}\n", superproject.display())),
            "submodule must be labeled as such: {}",
            out
        );
        assert!(
            !out.contains("worktree of:"),
            "submodule must not claim to be a worktree: {}",
            out
        );
    }

    #[test]
    fn test_worktree_of_a_submodule_keeps_worktree_linkage() {
        let (_guard, tmp) = tmpdir();
        let superproject = tmp.join("super");
        fs::create_dir_all(&superproject).expect("mkdir super");
        make_main_checkout(&superproject, "main");

        // Submodule gitdir with its own linked worktree
        let sub_gitdir = superproject.join(".git").join("modules").join("mylib");
        let wt_gitdir = sub_gitdir.join("worktrees").join("wtx");
        fs::create_dir_all(&wt_gitdir).expect("mkdir submodule worktree gitdir");
        fs::write(sub_gitdir.join("HEAD"), "ref: refs/heads/main\n").expect("write sub HEAD");
        fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feature\n").expect("write wt HEAD");
        fs::write(wt_gitdir.join("commondir"), "../..\n").expect("write commondir");

        let wt = tmp.join("wt-sub");
        fs::create_dir_all(&wt).expect("mkdir wt");
        fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_gitdir.display()))
            .expect("write .git pointer file");

        let out = describe(&wt, None);
        assert!(out.contains("branch: feature\n"), "{}", out);
        assert!(
            out.contains(&format!("worktree of: {} (branch main)\n", sub_gitdir.display())),
            "worktree linkage (the drift signal) must survive: {}",
            out
        );
        assert!(
            out.contains(&format!("submodule of: {}\n", superproject.display())),
            "submodule attribution must also be reported: {}",
            out
        );
    }

    #[test]
    fn test_submodule_at_worktrees_path_not_mislabeled() {
        let (_guard, tmp) = tmpdir();
        let superproject = tmp.join("super");
        let sub = superproject.join("worktrees").join("foo");
        fs::create_dir_all(&sub).expect("mkdir submodule");
        make_main_checkout(&superproject, "main");

        // Plain submodule at path `worktrees/foo`: gitdir parent is named
        // `worktrees` but there is no commondir — must stay a submodule.
        let gitdir = superproject
            .join(".git")
            .join("modules")
            .join("worktrees")
            .join("foo");
        fs::create_dir_all(&gitdir).expect("mkdir gitdir");
        fs::write(gitdir.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        fs::write(sub.join(".git"), format!("gitdir: {}\n", gitdir.display()))
            .expect("write .git pointer file");

        let out = describe(&sub, None);
        assert!(
            !out.contains("worktree of:"),
            "gitdir parent named 'worktrees' must not fake a worktree link: {}",
            out
        );
        assert!(
            out.contains(&format!("submodule of: {}\n", superproject.display())),
            "{}",
            out
        );
    }

    #[test]
    fn test_nested_submodule_labeled_as_nested() {
        let (_guard, tmp) = tmpdir();
        let superproject = tmp.join("super");
        let sub = superproject.join("a").join("b");
        fs::create_dir_all(&sub).expect("mkdir nested submodule");
        make_main_checkout(&superproject, "main");

        let gitdir = superproject
            .join(".git")
            .join("modules")
            .join("a")
            .join("modules")
            .join("b");
        fs::create_dir_all(&gitdir).expect("mkdir nested gitdir");
        fs::write(gitdir.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        fs::write(sub.join(".git"), format!("gitdir: {}\n", gitdir.display()))
            .expect("write .git pointer file");

        let out = describe(&sub, None);
        assert!(
            out.contains(&format!(
                "submodule (nested) of: {}\n",
                superproject.display()
            )),
            "nested submodule must flag top-level attribution: {}",
            out
        );
    }

    #[test]
    fn test_worktree_with_unreadable_main_head() {
        let (_guard, tmp) = tmpdir();
        let main = tmp.join("main-checkout");
        let wt = tmp.join("wt");
        fs::create_dir_all(&main).expect("mkdir main");
        make_main_checkout(&main, "develop");
        make_worktree(&main, &wt, "wt", "feature");
        fs::remove_file(main.join(".git/HEAD")).expect("drop main HEAD");

        let out = describe(&wt, None);
        assert!(
            out.contains(&format!("worktree of: {}\n", main.display())),
            "main link without branch suffix when its HEAD is unreadable: {}",
            out
        );
        assert!(!out.contains("(branch"), "{}", out);
    }

    #[test]
    fn test_non_branch_symbolic_ref_printed_verbatim() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("mkdir .git");
        fs::write(git.join("HEAD"), "ref: refs/remotes/origin/main\n").expect("write HEAD");

        let out = describe(&root, None);
        assert!(
            out.contains("branch: refs/remotes/origin/main\n"),
            "non-heads symbolic ref must print verbatim, not as detached garbage: {}",
            out
        );
    }

    #[test]
    fn test_garbage_head_degrades_to_unknown() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("mkdir .git");
        fs::write(git.join("HEAD"), "totally not a ref\n").expect("write HEAD");

        let out = describe(&root, None);
        assert!(out.contains("branch: (unknown)\n"), "{}", out);
    }

    #[test]
    fn test_malformed_gitdir_pointer() {
        let (_guard, tmp) = tmpdir();
        let wt = tmp.join("wt");
        fs::create_dir_all(&wt).expect("mkdir wt");
        fs::write(wt.join(".git"), "not a gitdir line\n").expect("write bogus .git");

        let out = describe(&wt, None);
        assert!(
            out.contains("branch: (unknown)"),
            "malformed pointer must degrade, not panic: {}",
            out
        );
    }
}
