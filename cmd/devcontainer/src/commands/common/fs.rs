//! Filesystem helpers shared across command implementations.

use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(io_error_to_string)
        .and_then(|()| copy_directory_entries(source, destination))
}

fn copy_directory_entries(source: &Path, destination: &Path) -> Result<(), String> {
    fs::read_dir(source)
        .map_err(io_error_to_string)
        .and_then(|entries| {
            entries
                .into_iter()
                .try_for_each(|entry| copy_directory_entry(entry, destination))
        })
}

fn copy_directory_entry(entry: io::Result<fs::DirEntry>, destination: &Path) -> Result<(), String> {
    entry.map_err(io_error_to_string).and_then(|entry| {
        let entry_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry_path.is_dir() {
            copy_directory_recursive(&entry_path, &destination_path)
        } else {
            fs::copy(&entry_path, &destination_path)
                .map(|_| ())
                .map_err(io_error_to_string)
        }
    })
}

fn io_error_to_string(error: io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::test_support::unique_temp_dir;

    use super::{copy_directory_entry, copy_directory_recursive};

    fn assert_error_contains_any(error: &str, needles: &[&str]) {
        let normalized = error.to_lowercase();
        assert!(
            needles.iter().any(|needle| normalized.contains(needle)),
            "expected {error:?} to contain one of {needles:?}"
        );
    }

    #[test]
    fn copy_directory_recursive_reports_source_read_errors() {
        let root = unique_temp_dir("copy-directory-read-error");
        let destination = root.join("dest");
        fs::create_dir_all(&root).expect("root");

        let error = copy_directory_recursive(&root.join("missing"), &destination)
            .expect_err("missing source should fail");

        assert_error_contains_any(&error, &["no such file", "not find"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_directory_recursive_reports_destination_create_errors() {
        let root = unique_temp_dir("copy-directory-create-error");
        let source = root.join("source");
        let destination_parent_file = root.join("dest-parent");
        fs::create_dir_all(&source).expect("source");
        fs::write(&destination_parent_file, "not a directory").expect("parent file");

        let error = copy_directory_recursive(&source, &destination_parent_file.join("dest"))
            .expect_err("destination parent file should fail");

        assert_error_contains_any(&error, &["not a directory", "exists"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_directory_recursive_reports_nested_directory_create_errors() {
        let root = unique_temp_dir("copy-directory-nested-create-error");
        let source = root.join("source");
        let destination = root.join("dest");
        fs::create_dir_all(source.join("nested")).expect("nested source");
        fs::create_dir_all(&destination).expect("dest");
        fs::write(destination.join("nested"), "not a directory").expect("dest file");

        let error = copy_directory_recursive(&source, &destination)
            .expect_err("destination child file should block nested copy");

        assert_error_contains_any(&error, &["not a directory", "exists"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_directory_recursive_reports_file_copy_errors() {
        let root = unique_temp_dir("copy-directory-copy-error");
        let source = root.join("source");
        let destination = root.join("dest");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(destination.join("file.txt")).expect("destination collision");
        fs::write(source.join("file.txt"), "contents").expect("source file");

        let error = copy_directory_recursive(&source, &destination)
            .expect_err("copying a file over a directory should fail");

        assert_error_contains_any(&error, &["is a directory", "access is denied"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_directory_entry_reports_iterator_errors() {
        let error = copy_directory_entry(
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            std::path::Path::new("/unused"),
        )
        .expect_err("iterator errors should be propagated");

        assert_error_contains_any(&error, &["permission", "denied"]);
    }
}
