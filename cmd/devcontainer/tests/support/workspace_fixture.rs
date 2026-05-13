//! Reusable workspace setup helpers for runtime and CLI smoke tests.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) struct WorkspaceFixture {
    root: PathBuf,
}

impl WorkspaceFixture {
    pub(crate) fn new(root: PathBuf) -> Self {
        fs::create_dir_all(&root).expect("workspace dir");
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn config_dir(&self) -> PathBuf {
        self.root.join(".devcontainer")
    }

    pub(crate) fn write_devcontainer_config(&self, body: &str) -> PathBuf {
        let config_dir = self.config_dir();
        fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("devcontainer.json");
        fs::write(&config_path, body).expect("config write");
        config_path
    }

    pub(crate) fn create_dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).expect("dir");
        path
    }

    pub(crate) fn write_file(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(&path, contents).expect("file write");
        path
    }

    pub(crate) fn create_local_feature(
        &self,
        name: &str,
        manifest: &str,
        install_script: &str,
    ) -> PathBuf {
        let feature_dir = self.create_dir(&format!(".devcontainer/{name}"));
        fs::write(feature_dir.join("devcontainer-feature.json"), manifest).expect("manifest");
        fs::write(feature_dir.join("install.sh"), install_script).expect("install script");
        feature_dir
    }

    pub(crate) fn init_dotfiles_repo(&self, relative: &str, install_script: &str) -> PathBuf {
        let repo = self.create_dir(relative);
        fs::write(repo.join("install.sh"), install_script).expect("install script");
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "user.name", "Codex Test"]);
        run_git(&repo, &["config", "user.email", "codex@example.com"]);
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-qm", "init"]);
        repo
    }

    pub(crate) fn init_git_repo(&self, relative: &str) -> PathBuf {
        let repo = self.create_dir(relative);
        run_git(&repo, &["init", "--quiet"]);
        repo
    }

    pub(crate) fn init_git_repo_with_commit(&self, relative: &str) -> PathBuf {
        let repo = self.init_git_repo(relative);
        fs::write(repo.join("README.md"), "hello\n").expect("readme");
        run_git(&repo, &["add", "README.md"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Devcontainer Tests",
                "-c",
                "user.email=devcontainer-tests@example.com",
                "commit",
                "--quiet",
                "-m",
                "init",
            ],
        );
        repo
    }

    pub(crate) fn add_relative_git_worktree(
        &self,
        repo_relative: &str,
        worktree_relative: &str,
        branch: &str,
    ) -> PathBuf {
        let repo_root = self.root.join(repo_relative);
        let worktree_root = self.root.join(worktree_relative);
        if let Some(parent) = worktree_root.parent() {
            fs::create_dir_all(parent).expect("worktree parent");
        }
        run_git(
            &repo_root,
            &[
                "worktree",
                "add",
                "--relative-paths",
                worktree_root.to_string_lossy().as_ref(),
                "-b",
                branch,
            ],
        );
        worktree_root
    }
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git command");
    assert!(
        status.success(),
        "git {:?} failed in {}",
        args,
        cwd.display()
    );
}
