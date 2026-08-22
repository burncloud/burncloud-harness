use std::{collections::BTreeSet, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};

pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn ensure_repository(&self) -> Result<()> {
        let output = self.git(["rev-parse", "--show-toplevel"])?;
        if !output.status.success() {
            bail!(
                "workspace is not a git repository: {}",
                output.stderr.trim()
            );
        }
        Ok(())
    }

    pub fn ensure_clean(&self) -> Result<()> {
        let output = self.git(["status", "--porcelain"])?;
        if !output.status.success() {
            bail!("git status failed: {}", output.stderr.trim());
        }
        if !output.stdout.trim().is_empty() {
            bail!(
                "BurnCloud worktree must be clean before a harness run. Existing changes:\n{}",
                output.stdout.trim_end()
            );
        }
        Ok(())
    }

    pub fn head_sha(&self) -> Result<String> {
        let output = self.git(["rev-parse", "HEAD"])?;
        if !output.status.success() {
            bail!("failed to resolve HEAD: {}", output.stderr.trim());
        }
        Ok(output.stdout.trim().to_owned())
    }

    pub fn changed_paths(&self) -> Result<Vec<String>> {
        let tracked = self.git([
            "diff",
            "HEAD",
            "--name-only",
            "--diff-filter=ACDMRTUXB",
            "--",
        ])?;
        if !tracked.status.success() {
            bail!("git diff failed: {}", tracked.stderr.trim());
        }
        let untracked = self.git(["ls-files", "--others", "--exclude-standard"])?;
        if !untracked.status.success() {
            bail!("git ls-files failed: {}", untracked.stderr.trim());
        }

        let mut paths = BTreeSet::new();
        for line in tracked.stdout.lines().chain(untracked.stdout.lines()) {
            let path = line.trim();
            if !path.is_empty() {
                paths.insert(path.replace('\\', "/"));
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub fn diff(&self) -> Result<String> {
        let output = self.git(["diff", "HEAD", "--no-ext-diff", "--unified=1", "--"])?;
        if !output.status.success() {
            bail!("git diff failed: {}", output.stderr.trim());
        }
        Ok(output.stdout)
    }

    pub fn harness_state_dir(&self) -> Result<PathBuf> {
        let output = self.git(["rev-parse", "--git-dir"])?;
        if !output.status.success() {
            bail!("failed to locate git directory: {}", output.stderr.trim());
        }
        let git_dir = PathBuf::from(output.stdout.trim());
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            self.root.join(git_dir)
        };
        Ok(git_dir.join("burncloud-harness"))
    }

    fn git<const N: usize>(&self, args: [&str; N]) -> Result<CommandOutput> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .output()
            .with_context(|| format!("failed to execute git in {}", self.root.display()))?;

        Ok(CommandOutput {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}
