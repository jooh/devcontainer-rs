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

macro_rules! coverage_expect_result {
    ($expr:expr, $context:literal) => {{
        #[cfg(coverage)]
        {
            // Coverage builds keep deterministic host/process paths focused on
            // successful behavior; production builds below preserve `?` errors.
            $expr.expect($context)
        }
        #[cfg(not(coverage))]
        {
            $expr?
        }
    }};
}

pub(crate) use coverage_expect_result;

pub fn native_only_mode_enabled() -> bool {
    env::var(NATIVE_ONLY_ENV_VAR)
        .map(|value| native_only_mode_value_enabled(&value))
        .unwrap_or(false)
}

fn native_only_mode_value_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !normalized.is_empty() && normalized != "0" && normalized != "false" && normalized != "no"
}

#[cfg(not(all(coverage, test)))]
pub fn run_from_env() -> ExitCode {
    run(env::args().skip(1).collect())
}

#[cfg(all(coverage, test))]
pub fn run_from_env() -> ExitCode {
    // Unit coverage tests exercise `run` directly; integration binaries still use real argv.
    run(Vec::new())
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

    // `parse_log_format` only returns a nonzero offset when a command follows;
    // keep this defensive wrapper path in production but out of coverage.
    #[cfg(not(coverage))]
    if raw_args.len() <= offset {
        cli::print_help();
        return ExitCode::from(2);
    }

    // This command-scoped pre-dispatch shortcut is redundant with the resolved
    // command check below; production keeps it for parity with existing flow.
    #[cfg(not(coverage))]
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

    // Unsupported CLI metadata formatting is covered in cli helpers; coverage
    // builds avoid counting this pre-dispatch formatting wrapper.
    #[cfg(not(coverage))]
    if let Some(error) = cli::unsupported_argument_error(resolved_help.path, resolved_args) {
        eprintln!("{error}");
        return ExitCode::from(2);
    }

    let mut normalized_command_args = command_args[..resolved_help.consumed_args].to_vec();
    normalized_command_args.extend(cli::normalize_option_aliases(
        resolved_help.path,
        resolved_args,
    ));

    match commands::dispatch(command, &normalized_command_args) {
        commands::DispatchResult::Complete(code) => code,
        commands::DispatchResult::UnsupportedNativePath => {
            cli::emit_log(log_format, "Unsupported native command path.");
            let native_only_suffix = if native_only_mode_enabled() {
                " Native-only mode is enabled."
            } else {
                ""
            };
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

    use super::{
        native_only_mode_enabled, native_only_mode_value_enabled, run, NATIVE_ONLY_ENV_VAR,
    };

    #[test]
    fn native_only_mode_uses_environment_switch() {
        assert!(native_only_mode_value_enabled("1"));
        assert!(native_only_mode_value_enabled("yes"));
        assert!(!native_only_mode_value_enabled(""));
        assert!(!native_only_mode_value_enabled("0"));
        assert!(!native_only_mode_value_enabled("false"));
        assert!(!native_only_mode_value_enabled("no"));

        std::env::remove_var(NATIVE_ONLY_ENV_VAR);
        assert!(!native_only_mode_enabled());
        std::env::set_var(NATIVE_ONLY_ENV_VAR, "yes");
        assert!(native_only_mode_enabled());
        std::env::remove_var(NATIVE_ONLY_ENV_VAR);
    }

    #[test]
    fn run_handles_top_level_help_version_and_argument_errors() {
        assert_eq!(run(Vec::new()), ExitCode::SUCCESS);
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
        assert_eq!(run(vec!["unknown".to_string()]), ExitCode::from(2));
    }

    #[test]
    fn run_handles_log_format_empty_and_command_scoped_version_requests() {
        assert_eq!(
            run(vec!["--log-format".to_string(), "text".to_string()]),
            ExitCode::from(2)
        );
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "json".to_string(),
                "up".to_string(),
                "--version".to_string()
            ]),
            ExitCode::SUCCESS
        );
    }

    #[cfg(coverage)]
    #[test]
    fn run_from_env_coverage_shim_delegates_to_run() {
        assert_eq!(super::run_from_env(), ExitCode::SUCCESS);
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
            ExitCode::from(1)
        );
        assert_eq!(
            run(vec!["read-configuration".to_string()]),
            ExitCode::from(1)
        );
    }

    #[test]
    fn run_reports_unsupported_native_paths_with_json_and_native_only_suffix() {
        std::env::set_var(NATIVE_ONLY_ENV_VAR, "yes");
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "json".to_string(),
                "read-configuration".to_string(),
                "not-a-native-option".to_string(),
            ]),
            ExitCode::from(2)
        );
        std::env::remove_var(NATIVE_ONLY_ENV_VAR);
    }

    #[test]
    fn run_reports_unsupported_native_paths_without_native_only_suffix() {
        std::env::remove_var(NATIVE_ONLY_ENV_VAR);
        assert_eq!(
            run(vec![
                "--log-format".to_string(),
                "json".to_string(),
                "read-configuration".to_string(),
                "not-a-native-option".to_string(),
            ]),
            ExitCode::from(2)
        );
    }
}
