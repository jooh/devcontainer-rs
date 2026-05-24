//! Shared path and temporary-path helpers for runtime code.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP_PATH_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn resolve_relative(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

pub(crate) fn unique_temp_path(prefix: &str, extension: Option<&str>) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let unique_id = NEXT_TEMP_PATH_ID.fetch_add(1, Ordering::Relaxed);
    let extension = extension.unwrap_or_default();
    let extension = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{suffix}-{unique_id}{extension}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{resolve_relative, unique_temp_path};

    #[test]
    fn unique_temp_path_uses_requested_extension() {
        let path = unique_temp_path("devcontainer-path-test", Some("yml"));

        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml")
        );
        assert!(path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("devcontainer-path-test-")));
    }

    #[test]
    fn unique_temp_path_omits_empty_extension() {
        let path = unique_temp_path("devcontainer-path-test", None);

        assert_eq!(path.extension(), None);
        assert!(path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| !name.ends_with('.')));
    }

    #[test]
    fn resolve_relative_preserves_absolute_paths() {
        assert_eq!(
            resolve_relative(Path::new("/workspace"), "/already/absolute"),
            Path::new("/already/absolute")
        );
        assert_eq!(
            resolve_relative(Path::new("/workspace"), "relative/path"),
            Path::new("/workspace").join("relative/path")
        );
    }
}
