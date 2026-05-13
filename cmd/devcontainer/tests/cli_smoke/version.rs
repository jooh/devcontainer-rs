//! CLI smoke tests for top-level version output.

use devcontainer::VERSION;

use crate::support::test_support::devcontainer_command;

#[test]
fn top_level_version_flags_print_the_package_version() {
    let output = devcontainer_command(None)
        .arg("--version")
        .output()
        .expect("version command should run");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        format!("{VERSION}\n")
    );
    assert_eq!(String::from_utf8(output.stderr).expect("utf8 stderr"), "");
}

#[test]
fn command_scoped_version_flags_print_the_package_version() {
    for args in [
        ["up", "--version"].as_slice(),
        ["features", "--version"].as_slice(),
        ["templates", "apply", "--version"].as_slice(),
    ] {
        let output = devcontainer_command(None)
            .args(args)
            .output()
            .expect("version command should run");

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("utf8 stdout"),
            format!("{VERSION}\n")
        );
        assert_eq!(String::from_utf8(output.stderr).expect("utf8 stderr"), "");
    }
}
