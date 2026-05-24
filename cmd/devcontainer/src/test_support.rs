//! Shared helpers for crate-internal unit tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);
static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn process_env_lock() -> MutexGuard<'static, ()> {
    PROCESS_ENV_LOCK.lock().expect("process env lock")
}

pub(crate) struct ProcessEnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(OsString, Option<OsString>)>,
}

impl ProcessEnvGuard {
    pub(crate) fn set_var(&mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let name = name.as_ref().to_os_string();
        self.save_original(&name);
        std::env::set_var(&name, value);
    }

    pub(crate) fn remove_var(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref().to_os_string();
        self.save_original(&name);
        std::env::remove_var(&name);
    }

    fn save_original(&mut self, name: &OsStr) {
        for (saved_name, _) in &self.saved {
            if saved_name.as_os_str() == name {
                return;
            }
        }
        self.saved
            .push((name.to_os_string(), std::env::var_os(name)));
    }
}

impl Drop for ProcessEnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.iter().rev() {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

pub(crate) fn process_env_guard() -> ProcessEnvGuard {
    ProcessEnvGuard {
        _lock: process_env_lock(),
        saved: Vec::new(),
    }
}

pub(crate) fn unique_temp_dir(prefix: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let unique_id = NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{suffix}-{unique_id}",
        std::process::id()
    ))
}

pub(crate) fn init_git_repo(root: &Path) {
    run_git(root, &["init", "--quiet"]);
}

pub(crate) fn run_git(cwd: &Path, args: &[&str]) {
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

pub(crate) fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).expect("script");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("permissions");
}

pub(crate) fn write_test_control_manifest(user_data: &Path) {
    fs::create_dir_all(user_data).expect("user data dir");
    fs::write(
        user_data.join("control-manifest.json"),
        json!({
            "disallowedFeatures": [{
                "featureIdPrefix": "ghcr.io/devcontainers/features/problematic-feature",
                "documentationURL": "https://containers.dev/"
            }],
            "featureAdvisories": [{
                "featureId": "ghcr.io/devcontainers/features/feature-with-advisory",
                "introducedInVersion": "1.0.7",
                "fixedInVersion": "1.1.10",
                "description": "Fixture advisory entry for native parity testing.",
                "documentationURL": "https://containers.dev/"
            }]
        })
        .to_string(),
    )
    .expect("control manifest");
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{process_env_guard, run_git, unique_temp_dir};

    #[test]
    fn run_git_reports_command_failures_with_working_directory() {
        let root = unique_temp_dir("devcontainer-test-support-git-failure");
        fs::create_dir_all(&root).expect("root dir");

        let panic = std::panic::catch_unwind(|| {
            run_git(&root, &["definitely-not-a-git-command"]);
        });

        assert!(panic.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn process_env_guard_restores_existing_variables() {
        let name = format!(
            "DEVCONTAINER_TEST_SUPPORT_ENV_{}",
            unique_temp_dir("env")
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        std::env::set_var(&name, "original");

        {
            let mut guard = process_env_guard();
            guard.set_var(&name, "changed");
            guard.set_var(&name, "changed-again");
            assert_eq!(std::env::var(&name).as_deref(), Ok("changed-again"));
        }

        assert_eq!(std::env::var(&name).as_deref(), Ok("original"));
        std::env::remove_var(name);
    }

    #[test]
    fn process_env_guard_restores_removed_variables() {
        let name = format!(
            "DEVCONTAINER_TEST_SUPPORT_ENV_{}",
            unique_temp_dir("removed-env")
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        std::env::remove_var(&name);

        {
            let mut guard = process_env_guard();
            guard.set_var(&name, "temporary");
            assert_eq!(std::env::var(&name).as_deref(), Ok("temporary"));
            guard.remove_var(&name);
            assert!(std::env::var(&name).is_err());
        }

        assert!(std::env::var(&name).is_err());
    }
}
