//! Shared command-line parsing and filesystem helpers used across commands.

mod args;
mod config_resolution;
mod fs;
mod labels;
mod manifest;

pub(crate) use args::{
    has_flag, parse_bool_option, parse_json_string_array_option, parse_option_value,
    parse_option_values, remote_env_overrides, runtime_options, runtime_process_request,
    secrets_env, validate_choice_option, validate_number_option, validate_option_values,
    validate_paired_options,
};
pub(crate) use config_resolution::{
    load_resolved_config, load_resolved_config_with_id_labels, resolve_override_config_path,
    resolve_read_configuration_path,
};
pub(crate) use fs::{copy_directory_recursive, package_collection_target};
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
    }
}
