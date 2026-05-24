//! Exec command entrypoint and result formatting helpers.

use std::process::ExitCode;

use crate::runtime;

pub(crate) fn run(args: &[String]) -> ExitCode {
    exit_code_for_result(runtime::run_exec(args))
}

fn exit_code_for_result(result: Result<i32, String>) -> ExitCode {
    match result {
        Ok(status_code) => ExitCode::from(status_code as u8),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{exit_code_for_result, run};

    #[test]
    fn maps_runtime_status_to_exit_code() {
        assert_eq!(exit_code_for_result(Ok(7)), ExitCode::from(7));
    }

    #[test]
    fn maps_runtime_errors_to_failure() {
        assert_eq!(
            exit_code_for_result(Err("missing command".to_string())),
            ExitCode::from(1)
        );
    }

    #[test]
    fn run_reports_missing_exec_command() {
        assert_eq!(run(&[]), ExitCode::from(1));
    }
}
