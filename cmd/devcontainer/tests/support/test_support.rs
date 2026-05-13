#![allow(dead_code)]

//! Shared temporary-directory and binary lookup helpers for integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

pub(crate) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

pub(crate) fn copy_recursive(source: &Path, destination: &Path) {
    let metadata = fs::metadata(source).expect("metadata");
    if metadata.is_dir() {
        fs::create_dir_all(destination).expect("create dir");
        for entry in fs::read_dir(source).expect("read dir") {
            let entry = entry.expect("dir entry");
            copy_recursive(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::copy(source, destination).expect("copy file");
    }
}

pub(crate) fn devcontainer_command(cwd: Option<&Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devcontainer"));
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
}
