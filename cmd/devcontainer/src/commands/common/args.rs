//! Shared command-line parsing and runtime option helpers.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::config;
use crate::process_runner::{ProcessLogLevel, ProcessRequest};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuntimeOptions {
    pub(crate) log_level: ProcessLogLevel,
    pub(crate) terminal_columns: Option<String>,
    pub(crate) terminal_rows: Option<String>,
    pub(crate) buildkit: Option<String>,
    pub(crate) gpu_availability: Option<String>,
    pub(crate) omit_syntax_directive: bool,
    pub(crate) omit_config_remote_env_from_metadata: bool,
    pub(crate) skip_persisting_customizations_from_features: bool,
    pub(crate) skip_feature_auto_mapping: bool,
    pub(crate) stop_for_personalization: bool,
    pub(crate) update_remote_user_uid_default: Option<String>,
    pub(crate) dotfiles_repository: Option<String>,
    pub(crate) dotfiles_install_command: Option<String>,
    pub(crate) dotfiles_target_path: Option<String>,
    pub(crate) user_data_folder: Option<String>,
    pub(crate) container_data_folder: Option<String>,
    pub(crate) container_system_data_folder: Option<String>,
    pub(crate) container_session_data_folder: Option<String>,
    pub(crate) default_user_env_probe: Option<String>,
}

pub(crate) fn parse_option_value(args: &[String], option: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == option)
        .map(|window| window[1].clone())
}

pub(crate) fn runtime_options(args: &[String]) -> RuntimeOptions {
    RuntimeOptions {
        log_level: match parse_option_value(args, "--log-level").as_deref() {
            Some("debug") => ProcessLogLevel::Debug,
            Some("trace") => ProcessLogLevel::Trace,
            _ => ProcessLogLevel::Info,
        },
        terminal_columns: parse_option_value(args, "--terminal-columns"),
        terminal_rows: parse_option_value(args, "--terminal-rows"),
        buildkit: parse_option_value(args, "--buildkit"),
        gpu_availability: parse_option_value(args, "--gpu-availability"),
        omit_syntax_directive: has_flag(args, "--omit-syntax-directive"),
        omit_config_remote_env_from_metadata: has_flag(
            args,
            "--omit-config-remote-env-from-metadata",
        ),
        skip_persisting_customizations_from_features: has_flag(
            args,
            "--skip-persisting-customizations-from-features",
        ),
        skip_feature_auto_mapping: parse_bool_option(args, "--skip-feature-auto-mapping", false),
        stop_for_personalization: parse_bool_option(args, "--stop-for-personalization", false),
        update_remote_user_uid_default: parse_option_value(
            args,
            "--update-remote-user-uid-default",
        ),
        dotfiles_repository: parse_option_value(args, "--dotfiles-repository"),
        dotfiles_install_command: parse_option_value(args, "--dotfiles-install-command"),
        dotfiles_target_path: parse_option_value(args, "--dotfiles-target-path"),
        user_data_folder: parse_option_value(args, "--user-data-folder"),
        container_data_folder: parse_option_value(args, "--container-data-folder"),
        container_system_data_folder: parse_option_value(args, "--container-system-data-folder"),
        container_session_data_folder: parse_option_value(args, "--container-session-data-folder"),
        default_user_env_probe: parse_option_value(args, "--default-user-env-probe"),
    }
}

pub(crate) fn runtime_process_request(
    args: &[String],
    program: String,
    request_args: Vec<String>,
    cwd: Option<PathBuf>,
) -> ProcessRequest {
    let options = runtime_options(args);
    let mut env = HashMap::new();
    if let (Some(columns), Some(rows)) = (
        options.terminal_columns.clone(),
        options.terminal_rows.clone(),
    ) {
        env.insert("COLUMNS".to_string(), columns);
        env.insert("LINES".to_string(), rows);
    }

    ProcessRequest {
        program,
        args: request_args,
        cwd,
        env,
        log_level: options.log_level,
    }
}

pub(crate) fn parse_bool_option(args: &[String], option: &str, default: bool) -> bool {
    const FALSE_VALUES: &[&str] = &["false", "0", "no", "off"];
    const TRUE_VALUES: &[&str] = &["true", "1", "yes", "on"];

    let Some(index) = args.iter().position(|arg| arg == option) else {
        return default;
    };
    match args.get(index + 1).map(String::as_str) {
        Some(next) if FALSE_VALUES.contains(&next) => false,
        Some(next) if TRUE_VALUES.contains(&next) => true,
        Some(next) if next.starts_with("--") => true,
        Some(_) => true,
        None => true,
    }
}

pub(crate) fn validate_option_values(args: &[String], options: &[&str]) -> Result<(), String> {
    for (index, arg) in args.iter().enumerate() {
        if options.contains(&arg.as_str())
            && args
                .get(index + 1)
                .is_none_or(|next| next.starts_with("--"))
        {
            return Err(format!("Missing value for option: {arg}"));
        }
    }

    Ok(())
}

pub(crate) fn validate_choice_option(
    args: &[String],
    option: &str,
    choices: &[&str],
) -> Result<(), String> {
    validate_option_values(args, &[option])
        .and_then(|()| validate_choice_values(args, option, choices))
}

fn validate_choice_values(args: &[String], option: &str, choices: &[&str]) -> Result<(), String> {
    parse_option_values(args, option)
        .into_iter()
        .find(|value| !choices.contains(&value.as_str()))
        .map_or(Ok(()), |value| {
            Err(format!(
                "Invalid value for option {option}: {value}. Expected one of: {}",
                choices.join(", ")
            ))
        })
}

pub(crate) fn validate_number_option(args: &[String], option: &str) -> Result<(), String> {
    validate_option_values(args, &[option]).and_then(|()| validate_number_values(args, option))
}

fn validate_number_values(args: &[String], option: &str) -> Result<(), String> {
    parse_option_values(args, option)
        .into_iter()
        .find(|value| value.parse::<f64>().is_err())
        .map_or(Ok(()), |value| {
            Err(format!(
                "Invalid value for option {option}: {value}. Expected a number."
            ))
        })
}

pub(crate) fn validate_paired_options(
    args: &[String],
    left: &str,
    right: &str,
) -> Result<(), String> {
    let has_left = has_flag(args, left);
    let has_right = has_flag(args, right);
    if has_left != has_right {
        return Err(format!("Options {left} and {right} must be used together."));
    }

    Ok(())
}

pub(crate) fn parse_option_values(args: &[String], option: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == option && index + 1 < args.len() {
            values.push(args[index + 1].clone());
            index += 2;
        } else {
            index += 1;
        }
    }
    values
}

pub(crate) fn parse_json_string_array_option(
    args: &[String],
    option: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = parse_option_value(args, option) else {
        return Ok(Vec::new());
    };
    let parsed = config::parse_jsonc_value(&value)?;
    let Some(array) = parsed.as_array() else {
        return Err(format!("{option} must be a JSON array"));
    };

    let mut values = Vec::with_capacity(array.len());
    for value in array {
        let Some(value) = value.as_str() else {
            return Err(format!("{option} entries must be strings"));
        };
        values.push(value.to_string());
    }
    Ok(values)
}

pub(crate) fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

pub(crate) fn parse_remote_env(args: &[String]) -> Map<String, Value> {
    parse_option_values(args, "--remote-env")
        .into_iter()
        .filter_map(|entry| {
            entry
                .split_once('=')
                .map(|(name, value)| (name.to_string(), Value::String(value.to_string())))
        })
        .collect()
}

pub(crate) fn remote_env_overrides(args: &[String]) -> HashMap<String, String> {
    parse_remote_env(args)
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

pub(crate) fn secrets_env(args: &[String]) -> Result<HashMap<String, String>, String> {
    let Some(path) = parse_option_value(args, "--secrets-file") else {
        return Ok(HashMap::new());
    };
    let raw = fs::read_to_string(&path).map_err(error_to_string)?;
    let parsed = config::parse_jsonc_value(&raw)?;
    let Some(entries) = parsed.as_object() else {
        return Err("--secrets-file must point to a JSON object".to_string());
    };
    Ok(entries
        .iter()
        .filter_map(|(key, value)| match value {
            Value::Null => None,
            Value::Bool(boolean) => Some((key.clone(), boolean.to_string())),
            Value::Number(number) => Some((key.clone(), number.to_string())),
            Value::String(text) => Some((key.clone(), text.clone())),
            _ => Some((key.clone(), value.to_string())),
        })
        .collect())
}

fn error_to_string(error: impl ToString) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    //! Unit tests for shared command-line parsing.

    use std::collections::HashMap;
    use std::fs;

    use serde_json::json;

    use crate::process_runner::ProcessLogLevel;
    use crate::test_support::unique_temp_dir;

    use super::{
        parse_bool_option, parse_json_string_array_option, parse_remote_env, remote_env_overrides,
        runtime_options, runtime_process_request, secrets_env, validate_choice_option,
        validate_number_option, validate_option_values, validate_paired_options,
    };

    #[test]
    fn runtime_options_collect_shared_runtime_flags() {
        let options = runtime_options(&[
            "--log-level".to_string(),
            "trace".to_string(),
            "--terminal-columns".to_string(),
            "120".to_string(),
            "--terminal-rows".to_string(),
            "40".to_string(),
            "--buildkit".to_string(),
            "never".to_string(),
            "--gpu-availability".to_string(),
            "all".to_string(),
            "--omit-syntax-directive".to_string(),
            "--omit-config-remote-env-from-metadata".to_string(),
            "--skip-persisting-customizations-from-features".to_string(),
            "--skip-feature-auto-mapping".to_string(),
            "--stop-for-personalization".to_string(),
            "--dotfiles-repository".to_string(),
            "./dotfiles".to_string(),
            "--dotfiles-install-command".to_string(),
            "install.sh".to_string(),
            "--dotfiles-target-path".to_string(),
            "./applied-dotfiles".to_string(),
            "--user-data-folder".to_string(),
            "/tmp/user-data".to_string(),
            "--container-data-folder".to_string(),
            "/tmp/container-data".to_string(),
            "--container-system-data-folder".to_string(),
            "/var/devcontainer".to_string(),
            "--container-session-data-folder".to_string(),
            "/tmp/session-data".to_string(),
            "--default-user-env-probe".to_string(),
            "loginShell".to_string(),
            "--update-remote-user-uid-default".to_string(),
            "off".to_string(),
        ]);

        assert_eq!(options.log_level, ProcessLogLevel::Trace);
        assert_eq!(options.terminal_columns.as_deref(), Some("120"));
        assert_eq!(options.terminal_rows.as_deref(), Some("40"));
        assert_eq!(options.buildkit.as_deref(), Some("never"));
        assert_eq!(options.gpu_availability.as_deref(), Some("all"));
        assert!(options.omit_syntax_directive);
        assert!(options.omit_config_remote_env_from_metadata);
        assert!(options.skip_persisting_customizations_from_features);
        assert!(options.skip_feature_auto_mapping);
        assert!(options.stop_for_personalization);
        assert_eq!(options.dotfiles_repository.as_deref(), Some("./dotfiles"));
        assert_eq!(
            options.dotfiles_install_command.as_deref(),
            Some("install.sh")
        );
        assert_eq!(
            options.dotfiles_target_path.as_deref(),
            Some("./applied-dotfiles")
        );
        assert_eq!(options.user_data_folder.as_deref(), Some("/tmp/user-data"));
        assert_eq!(
            options.container_data_folder.as_deref(),
            Some("/tmp/container-data")
        );
        assert_eq!(
            options.container_system_data_folder.as_deref(),
            Some("/var/devcontainer")
        );
        assert_eq!(
            options.container_session_data_folder.as_deref(),
            Some("/tmp/session-data")
        );
        assert_eq!(
            options.default_user_env_probe.as_deref(),
            Some("loginShell")
        );
        assert_eq!(
            options.update_remote_user_uid_default.as_deref(),
            Some("off")
        );
    }

    #[test]
    fn runtime_options_parse_explicit_false_for_hidden_runtime_flags() {
        let options = runtime_options(&[
            "--skip-feature-auto-mapping".to_string(),
            "false".to_string(),
            "--stop-for-personalization".to_string(),
            "0".to_string(),
        ]);

        assert!(!options.skip_feature_auto_mapping);
        assert!(!options.stop_for_personalization);
    }

    #[test]
    fn runtime_process_request_applies_terminal_env_and_log_level() {
        let request = runtime_process_request(
            &[
                "--terminal-columns".to_string(),
                "100".to_string(),
                "--terminal-rows".to_string(),
                "32".to_string(),
                "--log-level".to_string(),
                "debug".to_string(),
            ],
            "docker".to_string(),
            vec!["ps".to_string()],
            Some("/tmp/workspace".into()),
        );

        assert_eq!(request.program, "docker");
        assert_eq!(request.args, vec!["ps".to_string()]);
        assert_eq!(
            request.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/workspace"))
        );
        assert_eq!(
            request.env,
            HashMap::from([
                ("COLUMNS".to_string(), "100".to_string()),
                ("LINES".to_string(), "32".to_string()),
            ])
        );
        assert_eq!(request.log_level, ProcessLogLevel::Debug);
    }

    #[test]
    fn parse_bool_option_handles_defaults_and_present_values() {
        assert!(parse_bool_option(&[], "--flag", true));
        assert!(parse_bool_option(&["--flag".to_string()], "--flag", false));
        assert!(parse_bool_option(
            &["--flag".to_string(), "--next".to_string()],
            "--flag",
            false
        ));
        assert!(parse_bool_option(
            &["--flag".to_string(), "maybe".to_string()],
            "--flag",
            false
        ));
        assert!(parse_bool_option(
            &["--flag".to_string(), "on".to_string()],
            "--flag",
            false
        ));
    }

    #[test]
    fn option_value_validation_reports_missing_values_and_accepts_present_values() {
        let error = validate_option_values(&["--config".to_string()], &["--config"])
            .expect_err("missing value");
        assert!(error.contains("--config"));

        validate_option_values(
            &[
                "--config".to_string(),
                ".devcontainer/devcontainer.json".to_string(),
            ],
            &["--config"],
        )
        .expect("value present");
    }

    #[test]
    fn choice_options_reject_unknown_values() {
        let error = validate_choice_option(
            &["--log-level".to_string(), "warning".to_string()],
            "--log-level",
            &["info", "debug", "trace"],
        )
        .expect_err("invalid choice");

        assert!(error.contains("--log-level"));
        assert!(error.contains("warning"));
    }

    #[test]
    fn choice_options_accept_known_values() {
        validate_choice_option(
            &["--log-level".to_string(), "trace".to_string()],
            "--log-level",
            &["info", "debug", "trace"],
        )
        .expect("known choice");
    }

    #[test]
    fn number_options_require_numeric_values() {
        let error = validate_number_option(
            &["--terminal-columns".to_string(), "wide".to_string()],
            "--terminal-columns",
        )
        .expect_err("invalid number");

        assert!(error.contains("--terminal-columns"));
        assert!(error.contains("wide"));
    }

    #[test]
    fn number_options_accept_numeric_values() {
        validate_number_option(
            &["--terminal-columns".to_string(), "120".to_string()],
            "--terminal-columns",
        )
        .expect("numeric value");
    }

    #[test]
    fn paired_options_require_both_flags() {
        let error = validate_paired_options(
            &["--terminal-columns".to_string(), "120".to_string()],
            "--terminal-columns",
            "--terminal-rows",
        )
        .expect_err("missing paired option");

        assert!(error.contains("--terminal-columns"));
        assert!(error.contains("--terminal-rows"));
    }

    #[test]
    fn paired_options_accept_both_or_neither_flag() {
        validate_paired_options(&[], "--terminal-columns", "--terminal-rows")
            .expect("neither flag is valid");
        validate_paired_options(
            &[
                "--terminal-columns".to_string(),
                "120".to_string(),
                "--terminal-rows".to_string(),
                "40".to_string(),
            ],
            "--terminal-columns",
            "--terminal-rows",
        )
        .expect("both flags are valid");
    }

    #[test]
    fn json_string_array_options_parse_values_and_errors() {
        assert_eq!(
            parse_json_string_array_option(&[], "--features").expect("missing option"),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_json_string_array_option(
                &["--features".to_string(), "[\"a\", \"b\"]".to_string()],
                "--features",
            )
            .expect("array option"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(parse_json_string_array_option(
            &["--features".to_string(), "{}".to_string()],
            "--features",
        )
        .expect_err("non-array")
        .contains("JSON array"));
        assert!(parse_json_string_array_option(
            &["--features".to_string(), "[1]".to_string()],
            "--features",
        )
        .expect_err("non-string entry")
        .contains("entries must be strings"));
        assert!(parse_json_string_array_option(
            &["--features".to_string(), "{".to_string()],
            "--features",
        )
        .is_err());
    }

    #[test]
    fn remote_env_helpers_parse_name_value_entries() {
        let args = vec![
            "--remote-env".to_string(),
            "A=1".to_string(),
            "--remote-env".to_string(),
            "BROKEN".to_string(),
            "--remote-env".to_string(),
            "B=two".to_string(),
        ];

        assert_eq!(
            parse_remote_env(&args),
            json!({ "A": "1", "B": "two" }).as_object().unwrap().clone()
        );
        assert_eq!(
            remote_env_overrides(&args),
            HashMap::from([
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "two".to_string()),
            ])
        );
    }

    #[test]
    fn secrets_env_reads_json_objects_and_converts_values() {
        let root = unique_temp_dir("secrets-env");
        fs::create_dir_all(&root).expect("secrets root");
        let secrets_file = root.join("secrets.json");
        fs::write(
            &secrets_file,
            r#"{
                "NULL_VALUE": null,
                "BOOL_VALUE": true,
                "NUMBER_VALUE": 42,
                "STRING_VALUE": "secret",
                "OBJECT_VALUE": { "nested": true }
            }"#,
        )
        .expect("secrets file");

        let secrets = secrets_env(&[
            "--secrets-file".to_string(),
            secrets_file.display().to_string(),
        ])
        .expect("secrets");

        assert!(!secrets.contains_key("NULL_VALUE"));
        assert_eq!(secrets["BOOL_VALUE"], "true");
        assert_eq!(secrets["NUMBER_VALUE"], "42");
        assert_eq!(secrets["STRING_VALUE"], "secret");
        assert_eq!(secrets["OBJECT_VALUE"], "{\"nested\":true}");
        assert_eq!(
            secrets_env(&[]).expect("missing secrets flag"),
            HashMap::new()
        );

        fs::write(&secrets_file, "[]").expect("array secrets file");
        assert!(secrets_env(&[
            "--secrets-file".to_string(),
            secrets_file.display().to_string(),
        ])
        .expect_err("non-object secrets")
        .contains("JSON object"));

        fs::write(&secrets_file, "{").expect("invalid secrets file");
        assert!(secrets_env(&[
            "--secrets-file".to_string(),
            secrets_file.display().to_string(),
        ])
        .is_err());

        assert!(secrets_env(&[
            "--secrets-file".to_string(),
            root.join("missing.json").display().to_string(),
        ])
        .is_err());

        let _ = fs::remove_dir_all(root);
    }
}
