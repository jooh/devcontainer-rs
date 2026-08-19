//! Shared command-line parsing and filesystem helpers used across commands.

mod args;
mod config_resolution;
mod fs;
mod labels;
mod manifest;

#[cfg(not(target_os = "linux"))]
pub(crate) use args::DEVCONTAINER_WORKSPACE_MOUNT_CONSISTENCY;
pub(crate) use args::{
    config_option_value, env_default_bool_option, env_default_option_value, has_flag,
    parse_array_option_values, parse_json_string_array_option, parse_option_value,
    parse_option_values, remote_env_overrides, runtime_options, runtime_process_request,
    secrets_env, validate_choice_option, validate_number_option, validate_option_values,
    validate_paired_options, validate_runtime_env_defaults, DEVCONTAINER_DOCKER_COMPOSE_PATH,
    DEVCONTAINER_DOCKER_PATH, DEVCONTAINER_MOUNT_GIT_WORKTREE_COMMON_DIR,
    DEVCONTAINER_MOUNT_WORKSPACE_GIT_ROOT,
};
#[cfg(test)]
pub(crate) use args::{
    test_env_defaults, DEVCONTAINER_BUILDKIT, DEVCONTAINER_CONFIG,
    DEVCONTAINER_CONTAINER_DATA_FOLDER, DEVCONTAINER_GPU_AVAILABILITY,
    DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT, DEVCONTAINER_USER_DATA_FOLDER,
};
pub(crate) use config_resolution::{
    load_resolved_config, load_resolved_config_with_id_labels, resolve_override_config_path,
    resolve_read_configuration_path,
};
pub(crate) use fs::copy_directory_recursive;
pub(crate) use labels::{
    default_devcontainer_id_label_pairs, default_devcontainer_id_labels,
    normalize_devcontainer_label_path, normalize_devcontainer_label_path_for_platform,
    DEVCONTAINER_CONFIG_FILE_LABEL, DEVCONTAINER_LOCAL_FOLDER_LABEL,
};
pub(crate) use manifest::{generate_manifest_docs, parse_manifest, ManifestDocOptions};

pub(crate) fn feature_option_env_name(key: &str) -> String {
    let mut normalized = key
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();

    let leading_unsafe_len = normalized
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '_')
        .map(char::len_utf8)
        .sum::<usize>();
    if leading_unsafe_len > 0 {
        normalized.replace_range(..leading_unsafe_len, "_");
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::feature_option_env_name;

    #[test]
    fn feature_option_env_names_match_upstream_safe_id_cases() {
        assert_eq!(feature_option_env_name("option-name"), "OPTION_NAME");
        assert_eq!(
            feature_option_env_name("option1-name-with_dashes-"),
            "OPTION1_NAME_WITH_DASHES_"
        );
        assert_eq!(feature_option_env_name("myOptionName"), "MYOPTIONNAME");
        assert_eq!(feature_option_env_name("1name"), "_NAME");
        assert_eq!(feature_option_env_name("12345_option-name"), "_OPTION_NAME");
        assert_eq!(feature_option_env_name("!!!value"), "_VALUE");
    }
}
