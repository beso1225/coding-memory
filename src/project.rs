use crate::domain::hash;
use anyhow::{Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
pub struct Project {
    pub id: String,
    pub repository: PathBuf,
    pub root: PathBuf,
}
fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "Cannot resolve Git repository at {}",
        path.display()
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
pub fn resolve(path: &Path) -> Result<Project> {
    let repository = PathBuf::from(git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?)
    .canonicalize()?;
    let root = PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let id = hash(
        repository
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 repository path"))?
            .as_bytes(),
    );
    Ok(Project {
        id,
        repository,
        root,
    })
}
pub fn worktrees(path: &Path) -> Result<Vec<PathBuf>> {
    git(path, &["worktree", "list", "--porcelain"])?
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|s| Ok(PathBuf::from(s).canonicalize()?))
        .collect()
}
