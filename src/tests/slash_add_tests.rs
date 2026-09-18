use crate::ui::slash::add::resolve_path;
use std::path::PathBuf;

#[test]
fn test_resolve_path_absolute() {
    let result = resolve_path("/tmp/foo.txt");
    assert_eq!(result, PathBuf::from("/tmp/foo.txt"));
}

#[test]
fn test_resolve_path_relative_root() {
    let result = resolve_path("/");
    assert_eq!(result, PathBuf::from("/"));
}

#[test]
fn test_resolve_path_relative_is_under_cwd() {
    // CWD-reader: hold the shared CWD lock (see tests::acquire_cwd) so a
    // concurrent chdir by another test cannot move the directory — or delete
    // it — between `current_dir` and the assertion.
    let _lock = crate::tests::acquire_cwd();
    let result = resolve_path("bar.txt");
    let expected = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("bar.txt");
    assert_eq!(result, expected);
}

#[test]
fn test_resolve_path_empty_joins_cwd() {
    let _lock = crate::tests::acquire_cwd();
    let result = resolve_path("");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    assert_eq!(result, cwd);
}
