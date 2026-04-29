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
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !normalized.is_empty()
                && normalized != "0"
                && normalized != "false"
                && normalized != "no"
        })
        .unwrap_or(false)
}

pub fn run_from_env() -> ExitCode {
    run(env::args().skip(1).collect())
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
    use super::native_only_mode_enabled;

    #[test]
    fn native_only_mode_uses_environment_switch() {
        let original = std::env::var("DEVCONTAINER_NATIVE_ONLY").ok();
        std::env::set_var("DEVCONTAINER_NATIVE_ONLY", "1");
        assert!(native_only_mode_enabled());

        std::env::set_var("DEVCONTAINER_NATIVE_ONLY", "false");
        assert!(!native_only_mode_enabled());

        if let Some(value) = original {
            std::env::set_var("DEVCONTAINER_NATIVE_ONLY", value);
        } else {
            std::env::remove_var("DEVCONTAINER_NATIVE_ONLY");
        }
    }
}
