//! Top-level command routing for the devcontainer CLI.

mod collections;
pub(crate) mod common;
pub(crate) mod configuration;
mod exec;

use std::process::ExitCode;

use serde_json::Value;

use crate::runtime;

pub enum DispatchResult {
    Complete(ExitCode),
    UnsupportedNativePath,
}

pub(crate) fn dispatch(
    command: &str,
    args: &[String],
    options: common::OciAuthOptions,
) -> DispatchResult {
    if let Err(error) = collections::validate_oci_auth_options(&options) {
        eprintln!("{error}");
        return DispatchResult::Complete(ExitCode::from(2));
    }
    common::with_oci_auth_options(options, || dispatch_with_options(command, args))
}

fn dispatch_with_options(command: &str, args: &[String]) -> DispatchResult {
    match command {
        "read-configuration" => {
            if configuration::should_use_native_read_configuration(args) {
                DispatchResult::Complete(print_json_result_with_oci_auth_diagnostics(
                    configuration::build_read_configuration_payload(args),
                ))
            } else {
                DispatchResult::UnsupportedNativePath
            }
        }
        "build" => DispatchResult::Complete(print_json_result_with_oci_auth_diagnostics(
            runtime::run_build(args),
        )),
        "up" => DispatchResult::Complete(print_json_result_with_oci_auth_diagnostics(
            runtime::run_up(args),
        )),
        "set-up" => DispatchResult::Complete(print_json_result(runtime::run_set_up(args))),
        "run-user-commands" => {
            DispatchResult::Complete(print_json_result(runtime::run_user_commands(args)))
        }
        "outdated" => DispatchResult::Complete(configuration::run_outdated(args)),
        "upgrade" => DispatchResult::Complete(configuration::run_upgrade(args)),
        "exec" => DispatchResult::Complete(exec::run(args)),
        "features" => DispatchResult::Complete(collections::run_features(args)),
        "templates" => DispatchResult::Complete(collections::run_templates(args)),
        _ => DispatchResult::UnsupportedNativePath,
    }
}

fn print_json_result(result: Result<Value, String>) -> ExitCode {
    match result {
        Ok(mut payload) => {
            common::attach_oci_auth_diagnostics(&mut payload);
            println!("{payload}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn print_json_result_with_oci_auth_diagnostics(result: Result<Value, String>) -> ExitCode {
    common::mark_oci_auth_attempted();
    print_json_result(result)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::ExitCode;

    use serde_json::json;

    use crate::commands::common::OciAuthOptions;
    use crate::test_support::unique_temp_dir;

    use super::{dispatch, print_json_result, DispatchResult};

    fn complete_exit_code(result: DispatchResult) -> Option<ExitCode> {
        match result {
            DispatchResult::Complete(code) => Some(code),
            DispatchResult::UnsupportedNativePath => None,
        }
    }

    fn assert_complete_exit(command: &str, args: &[String], expected: ExitCode) {
        assert_eq!(
            complete_exit_code(dispatch(command, args, OciAuthOptions::default())),
            Some(expected),
            "{command} exit code"
        );
    }

    #[test]
    fn print_json_result_maps_success_and_error_results() {
        assert_eq!(
            print_json_result(Ok(json!({ "outcome": "success" }))),
            ExitCode::SUCCESS
        );
        assert_eq!(
            print_json_result(Err("failed".to_string())),
            ExitCode::from(1)
        );
    }

    #[test]
    fn dispatch_rejects_invalid_oci_auth_options() {
        assert_eq!(
            complete_exit_code(dispatch(
                "features",
                &[],
                OciAuthOptions {
                    hardening: false,
                    allowed_cross_origin_auth_hosts: vec![
                        "registry.example=auth.example".to_string(),
                    ],
                },
            )),
            Some(ExitCode::from(2))
        );
        assert_eq!(
            complete_exit_code(dispatch(
                "features",
                &[],
                OciAuthOptions {
                    hardening: true,
                    allowed_cross_origin_auth_hosts: vec!["invalid".to_string()],
                },
            )),
            Some(ExitCode::from(2))
        );
    }

    #[test]
    fn dispatch_routes_read_configuration_native_and_unsupported_paths() {
        let workspace = unique_temp_dir("dispatch-read-configuration");
        let config_dir = workspace.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("devcontainer.json"),
            r#"{ "image": "alpine" }"#,
        )
        .expect("config");

        assert_eq!(
            complete_exit_code(dispatch(
                "read-configuration",
                &[
                    "--workspace-folder".to_string(),
                    workspace.display().to_string()
                ],
                OciAuthOptions::default(),
            )),
            Some(ExitCode::SUCCESS)
        );
        assert!(matches!(
            dispatch(
                "read-configuration",
                &["--unsupported".to_string()],
                OciAuthOptions::default(),
            ),
            DispatchResult::UnsupportedNativePath
        ));
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn dispatch_routes_known_runtime_and_collection_commands() {
        for (command, args, expected) in [
            (
                "build",
                vec!["--no-lockfile".to_string(), "--frozen-lockfile".to_string()],
                ExitCode::from(1),
            ),
            (
                "up",
                vec!["--no-lockfile".to_string(), "--frozen-lockfile".to_string()],
                ExitCode::from(1),
            ),
            (
                "set-up",
                vec![
                    "--docker-path".to_string(),
                    "/bin/false".to_string(),
                    "--workspace-folder".to_string(),
                    "/missing-workspace".to_string(),
                ],
                ExitCode::from(1),
            ),
            (
                "run-user-commands",
                vec![
                    "--docker-path".to_string(),
                    "/bin/false".to_string(),
                    "--workspace-folder".to_string(),
                    "/missing-workspace".to_string(),
                ],
                ExitCode::from(1),
            ),
            (
                "outdated",
                vec!["--output-format".to_string(), "xml".to_string()],
                ExitCode::from(1),
            ),
            (
                "upgrade",
                vec![
                    "--feature".to_string(),
                    "ghcr.io/devcontainers/features/git".to_string(),
                ],
                ExitCode::from(1),
            ),
            ("exec", Vec::new(), ExitCode::from(1)),
            ("features", Vec::new(), ExitCode::from(1)),
            ("templates", Vec::new(), ExitCode::from(1)),
        ] {
            assert_complete_exit(command, &args, expected);
        }
    }

    #[test]
    fn dispatch_reports_unknown_commands_as_unsupported() {
        assert_eq!(
            complete_exit_code(dispatch("unknown", &[], OciAuthOptions::default())),
            None
        );
    }
}
