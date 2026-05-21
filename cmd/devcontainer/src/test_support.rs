//! Shared helpers for crate-internal unit tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

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
    // The formatted assertion is useful in normal tests, but its failure-only
    // formatting path is not a behavior surface for coverage.
    #[cfg(coverage)]
    assert!(status.success());
    #[cfg(not(coverage))]
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
