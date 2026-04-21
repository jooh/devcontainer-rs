//! Shared helpers for default devcontainer id-label generation and normalization.

use std::collections::HashMap;
use std::path::Path;

use super::args::parse_option_values;

pub(crate) const DEVCONTAINER_LOCAL_FOLDER_LABEL: &str = "devcontainer.local_folder";
pub(crate) const DEVCONTAINER_CONFIG_FILE_LABEL: &str = "devcontainer.config_file";

pub(crate) fn id_label_map(
    args: &[String],
    workspace_folder: &Path,
    config_file: &Path,
) -> HashMap<String, String> {
    let mut labels = parse_option_values(args, "--id-label")
        .into_iter()
        .filter_map(|entry| {
            entry
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect::<HashMap<_, _>>();
    if labels.is_empty() {
        labels.extend(default_devcontainer_id_label_pairs(
            workspace_folder,
            config_file,
        ));
    }
    labels
}

pub(crate) fn default_devcontainer_id_labels(
    workspace_folder: &Path,
    config_file: &Path,
) -> Vec<String> {
    default_devcontainer_id_label_pairs(workspace_folder, config_file)
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

pub(crate) fn default_devcontainer_id_label_pairs(
    workspace_folder: &Path,
    config_file: &Path,
) -> [(String, String); 2] {
    default_devcontainer_id_label_pairs_for_platform(
        std::env::consts::OS,
        workspace_folder,
        config_file,
    )
}

pub(crate) fn default_devcontainer_id_label_pairs_for_platform(
    platform: &str,
    workspace_folder: &Path,
    config_file: &Path,
) -> [(String, String); 2] {
    [
        (
            DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            normalize_devcontainer_label_path_for_platform(
                platform,
                &workspace_folder.display().to_string(),
            ),
        ),
        (
            DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
            normalize_devcontainer_label_path_for_platform(
                platform,
                &config_file.display().to_string(),
            ),
        ),
    ]
}

pub(crate) fn normalize_devcontainer_label_path(value: &str) -> String {
    normalize_devcontainer_label_path_for_platform(std::env::consts::OS, value)
}

pub(crate) fn normalize_devcontainer_label_path_for_platform(
    platform: &str,
    value: &str,
) -> String {
    if platform != "windows" {
        return value.to_string();
    }

    normalize_windows_label_path(value)
}

fn normalize_windows_label_path(value: &str) -> String {
    let value = value.replace('/', "\\");
    let bytes = value.as_bytes();
    let (prefix, rest, absolute) = if let Some(rest) = value.strip_prefix("\\\\") {
        ("\\\\".to_string(), rest, true)
    } else if bytes.len() >= 2 && bytes[1] == b':' {
        let drive = value[..1].to_ascii_lowercase();
        let rest = &value[2..];
        (format!("{drive}:"), rest, rest.starts_with('\\'))
    } else {
        (String::new(), value.as_str(), value.starts_with('\\'))
    };

    let mut segments = Vec::new();
    for segment in rest.split('\\') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if segments.last().is_some_and(|last| last != "..") {
                segments.pop();
            } else if !absolute {
                segments.push(segment.to_string());
            }
            continue;
        }
        segments.push(segment.to_string());
    }

    let mut normalized = prefix;
    if absolute && !normalized.ends_with('\\') {
        normalized.push('\\');
    }
    normalized.push_str(&segments.join("\\"));
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        default_devcontainer_id_label_pairs_for_platform,
        normalize_devcontainer_label_path_for_platform, DEVCONTAINER_CONFIG_FILE_LABEL,
        DEVCONTAINER_LOCAL_FOLDER_LABEL,
    };

    #[test]
    fn normalize_devcontainer_label_path_lowercases_windows_drive_letters() {
        assert_eq!(
            normalize_devcontainer_label_path_for_platform("windows", "C:\\CodeBlocks\\remill"),
            "c:\\CodeBlocks\\remill"
        );
    }

    #[test]
    fn normalize_devcontainer_label_path_normalizes_windows_separators_and_segments() {
        assert_eq!(
            normalize_devcontainer_label_path_for_platform(
                "windows",
                "C:/CodeBlocks/remill/.devcontainer/../devcontainer.json"
            ),
            "c:\\CodeBlocks\\remill\\devcontainer.json"
        );
    }

    #[test]
    fn default_devcontainer_id_labels_use_normalized_windows_paths() {
        let [(workspace_key, workspace_value), (config_key, config_value)] =
            default_devcontainer_id_label_pairs_for_platform(
                "windows",
                Path::new("C:/CodeBlocks/remill"),
                Path::new("C:/CodeBlocks/remill/.devcontainer/devcontainer.json"),
            );

        assert_eq!(workspace_key, DEVCONTAINER_LOCAL_FOLDER_LABEL);
        assert_eq!(workspace_value, "c:\\CodeBlocks\\remill");
        assert_eq!(config_key, DEVCONTAINER_CONFIG_FILE_LABEL);
        assert_eq!(
            config_value,
            "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"
        );
    }
}
