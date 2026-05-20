//! Exec command entrypoint and result formatting helpers.

use std::process::ExitCode;

use crate::runtime;

pub(crate) fn run(args: &[String]) -> ExitCode {
    match runtime::run_exec(args) {
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

    use super::run;

    #[test]
    fn exec_entrypoint_reports_runtime_errors() {
        assert_eq!(run(&[]), ExitCode::from(1));
    }
}
