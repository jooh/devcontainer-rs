#![forbid(unsafe_code)]

//! Crate entry points and shared module wiring for the native devcontainer CLI.

use std::env;
use std::process::ExitCode;

pub mod cli;
pub mod commands;
pub mod config;
pub mod output;
pub mod process_runner;
pub mod runtime;

#[cfg(test)]
pub(crate) mod test_support;

pub const NATIVE_ONLY_ENV_VAR: &str = "DEVCONTAINER_NATIVE_ONLY";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn native_only_mode_enabled() -> bool {
    env::var(NATIVE_ONLY_ENV_VAR)
        .map(|value| native_only_mode_value_enabled(&value))
        .unwrap_or(false)
}

fn native_only_mode_value_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !normalized.is_empty() && normalized != "0" && normalized != "false" && normalized != "no"
}

pub fn run_from_env() -> ExitCode {
    run(env::args().skip(1).collect())
}

fn unsupported_argument_exit_code(error: Option<String>) -> Option<ExitCode> {
    match error {
        Some(error) => {
            eprintln!("{error}");
            Some(ExitCode::from(2))
        }
        None => None,
    }
}

fn native_only_suffix(enabled: bool) -> &'static str {
    if enabled {
        " Native-only mode is enabled."
    } else {
        ""
    }
}

pub fn run(raw_args: Vec<String>) -> ExitCode {
    if raw_args.is_empty() || matches!(raw_args[0].as_str(), "--help" | "-h") {
        cli::print_help();
        return ExitCode::SUCCESS;
    }

    if cli::is_command_version_request(&raw_args) {
        println!("{VERSION}");
        return ExitCode::SUCCESS;
    }

    let (log_format, offset) = cli::parse_log_format(&raw_args);
    if !matches!(log_format, "text" | "json") {
        eprintln!("Unsupported log format: {log_format}");
        return ExitCode::from(2);
    }

    if raw_args.len() <= offset {
        cli::print_help();
        return ExitCode::from(2);
    }

    if cli::is_command_version_request(&raw_args[offset..]) {
        println!("{VERSION}");
        return ExitCode::SUCCESS;
    }

    let command = &raw_args[offset];
    if !cli::SUPPORTED_TOP_LEVEL_COMMANDS.contains(&command.as_str()) {
        eprintln!("Unsupported command: {command}");
        return ExitCode::from(2);
    }

    let command_args = &raw_args[offset + 1..];
    let resolved_help = cli::resolve_command_help(command, command_args).expect("known command");
    let resolved_args = &command_args[resolved_help.consumed_args..];

    if cli::is_command_help_request(resolved_args) {
        cli::print_command_help(resolved_help.path);
        return ExitCode::SUCCESS;
    }

    if cli::is_command_version_request(resolved_args) {
        println!("{VERSION}");
        return ExitCode::SUCCESS;
    }

    let mut normalized_command_args = command_args[..resolved_help.consumed_args].to_vec();
    normalized_command_args.extend(cli::normalize_option_aliases(
        resolved_help.path,
        resolved_args,
    ));

    if let Some(exit_code) = unsupported_argument_exit_code(cli::unsupported_argument_error(
        resolved_help.path,
        &normalized_command_args,
    )) {
        return exit_code;
    }

    match commands::dispatch(command, &normalized_command_args) {
        commands::DispatchResult::Complete(code) => code,
        commands::DispatchResult::UnsupportedNativePath => {
            cli::emit_log(log_format, "Unsupported native command path.");
            let native_only_suffix = native_only_suffix(native_only_mode_enabled());
            eprintln!(
                "Unsupported native command path: {command} {}{native_only_suffix}",
                command_args.join(" ")
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use crate::test_support::process_env_guard;

    use super::{
        native_only_mode_enabled, native_only_mode_value_enabled, native_only_suffix, run,
        run_from_env, unsupported_argument_exit_code, NATIVE_ONLY_ENV_VAR,
    };

    #[test]
    fn native_only_mode_uses_environment_switch() {
        assert!(native_only_mode_value_enabled("1"));
        assert!(native_only_mode_value_enabled("yes"));
        assert!(!native_only_mode_value_enabled(""));
        assert!(!native_only_mode_value_enabled("0"));
        assert!(!native_only_mode_value_enabled("false"));
        assert!(!native_only_mode_value_enabled("no"));
    }

    #[test]
    fn native_only_mode_reads_environment_switch() {
        let mut env = process_env_guard();
        env.set_var(NATIVE_ONLY_ENV_VAR, "yes");
        assert!(native_only_mode_enabled());
        env.set_var(NATIVE_ONLY_ENV_VAR, "0");
        assert!(!native_only_mode_enabled());
        env.remove_var(NATIVE_ONLY_ENV_VAR);
        assert!(!native_only_mode_enabled());
    }

    #[test]
    fn run_from_env_delegates_to_process_arguments() {
        let _ = run_from_env();
    }

    #[test]
    fn helper_branches_map_unsupported_arguments_and_native_only_suffixes() {
        assert_eq!(unsupported_argument_exit_code(None), None);
        assert_eq!(
            unsupported_argument_exit_code(Some("unsupported".to_string())),
            Some(ExitCode::from(2))
        );
        assert_eq!(native_only_suffix(true), " Native-only mode is enabled.");
        assert_eq!(native_only_suffix(false), "");
    }

    #[test]
    fn run_handles_top_level_help_version_and_argument_errors() {
        assert_eq!(run(Vec::new()), ExitCode::SUCCESS);
        assert_eq!(run(vec!["-h".to_string()]), ExitCode::SUCCESS);
        assert_eq!(run(vec!["--version".to_string()]), ExitCode::SUCCESS);
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "yaml".to_string(),
                "up".to_string()
            ]),
            ExitCode::from(2)
        );
        assert_eq!(
            run(vec!["--log-format".to_string(), "json".to_string()]),
            ExitCode::from(2)
        );
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "json".to_string(),
                "--version".to_string()
            ]),
            ExitCode::SUCCESS
        );
        assert_eq!(run(vec!["unknown".to_string()]), ExitCode::from(2));
    }

    #[test]
    fn run_handles_command_help_version_and_unsupported_native_paths() {
        assert_eq!(
            run(vec!["up".to_string(), "--help".to_string()]),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(vec!["up".to_string(), "--version".to_string()]),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(vec![
                "up".to_string(),
                "--definitely-unsupported".to_string()
            ]),
            ExitCode::from(2)
        );
        assert_eq!(
            run(vec!["read-configuration".to_string()]),
            ExitCode::from(1)
        );
        assert_eq!(
            run(vec![
                "read-configuration".to_string(),
                "unexpected-positional".to_string()
            ]),
            ExitCode::from(2)
        );
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "json".to_string(),
                "read-configuration".to_string(),
                "--user-data-folder".to_string(),
                "/tmp/devcontainer-user-data".to_string(),
            ]),
            ExitCode::from(2)
        );
    }
}
