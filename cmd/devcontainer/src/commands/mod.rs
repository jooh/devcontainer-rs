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

pub fn dispatch(command: &str, args: &[String]) -> DispatchResult {
    match command {
        "read-configuration" => {
            if configuration::should_use_native_read_configuration(args) {
                DispatchResult::Complete(print_json_result(
                    configuration::build_read_configuration_payload(args),
                ))
            } else {
                DispatchResult::UnsupportedNativePath
            }
        }
        "build" => DispatchResult::Complete(print_json_result(runtime::run_build(args))),
        "up" => DispatchResult::Complete(print_json_result(runtime::run_up(args))),
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
        Ok(payload) => {
            println!("{payload}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dispatch, DispatchResult};

    #[test]
    fn dispatch_reports_unknown_commands_as_unsupported_native_paths() {
        assert!(matches!(
            dispatch("unknown-command", &[]),
            DispatchResult::UnsupportedNativePath
        ));
    }
}
