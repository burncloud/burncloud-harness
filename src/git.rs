use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};

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

        let mut paths = BTreeSet::new();
        for line in tracked.stdout.lines() {
            let path = normalize_path(line);
            if !path.is_empty() {
                paths.insert(path);
            }
        }
        for path in self.untracked_paths()? {
            paths.insert(path);
        }
        Ok(paths.into_iter().collect())
    }

    pub fn diff(&self) -> Result<String> {
        let output = self.git(["diff", "HEAD", "--no-ext-diff", "--unified=1", "--"])?;
        if !output.status.success() {
            bail!("git diff failed: {}", output.stderr.trim());
        }

        let mut diff = output.stdout;
        for path in self.untracked_paths()? {
            append_untracked_file(&mut diff, &self.root, &path)?;
        }
        Ok(diff)
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

    fn untracked_paths(&self) -> Result<Vec<String>> {
        let output = self.git(["ls-files", "--others", "--exclude-standard"])?;
        if !output.status.success() {
            bail!("git ls-files failed: {}", output.stderr.trim());
        }

        Ok(output
            .stdout
            .lines()
            .map(normalize_path)
            .filter(|path| !path.is_empty())
            .collect())
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

fn append_untracked_file(diff: &mut String, root: &std::path::Path, path: &str) -> Result<()> {
    let full = root.join(path);
    let bytes = fs::read(&full)
        .with_context(|| format!("failed to read untracked file {}", full.display()))?;

    diff.push_str(&format!("\ndiff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"));

    match String::from_utf8(bytes) {
        Ok(content) => {
            let line_count = content.lines().count();
            diff.push_str(&format!("@@ -0,0 +1,{line_count} @@\n"));
            for line in content.lines() {
                diff.push('+');
                diff.push_str(line);
                diff.push('\n');
            }
        }
        Err(_) => diff.push_str("Binary files /dev/null and b/untracked differ\n"),
    }

    Ok(())
}

fn normalize_path(path: &str) -> String {
    path.trim().replace('\\', "/")
}

struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}
