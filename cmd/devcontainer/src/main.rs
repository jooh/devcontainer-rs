#![forbid(unsafe_code)]

//! Binary entrypoint for the native devcontainer CLI.

fn main() -> std::process::ExitCode {
    devcontainer::run_from_env()
}
