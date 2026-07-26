//! End-to-end conformance fixtures for checked origins and executable references.

use mojito::Compiler;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures(category: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(category),
    )
    .expect("origin fixture directory exists")
    .map(|entry| entry.expect("fixture entry").path())
    .filter(|path| {
        path.extension()
            .is_some_and(|extension| extension == "mojo")
    })
    .collect();
    paths.sort();
    paths
}

fn expected(source: &str) -> Option<&str> {
    source
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("# expect:").map(str::trim))
}

#[test]
fn origin_ok_fixtures_execute() {
    for path in fixtures("origin_ok") {
        let compiler = Compiler::default();
        let program = compiler
            .compile_path(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        compiler
            .execute(&program)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn origin_error_fixtures_are_rejected() {
    for path in fixtures("origin_error") {
        let source = fs::read_to_string(&path).expect("read origin fixture");
        let message = Compiler::default()
            .compile_path(&path)
            .expect_err("origin error fixture must fail compilation")
            .to_string();
        if let Some(expected) = expected(&source) {
            assert!(
                message.contains(expected),
                "{}: expected '{expected}' in '{message}'",
                path.display()
            );
        }
    }
}
