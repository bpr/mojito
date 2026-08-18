use std::io::Write;
use std::process::{Command, Stdio};

fn mojito() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mojito"))
}

#[test]
fn emit_mir_file_writes_only_a_canonical_artifact() {
    let output = mojito()
        .args(["emit-mir", "assets/ok/defines_main.mojo"])
        .output()
        .expect("run emit-mir");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with(b"mojito-mir 1.0\n"));
    assert!(output.stdout.ends_with(b"\n"));
    assert!(!output.stdout.ends_with(b"\n\n"));
}

#[test]
fn emit_mir_stdin_pipes_into_exec() {
    let mut emit = mojito()
        .arg("emit-mir")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn emit-mir");
    emit.stdin
        .take()
        .expect("emit stdin")
        .write_all(b"def main():\n    print(42)\n")
        .expect("write source");
    let emitted = emit.wait_with_output().expect("wait for emit-mir");
    assert!(emitted.status.success());

    let mut exec = mojito()
        .args(["exec", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn exec");
    exec.stdin
        .take()
        .expect("exec stdin")
        .write_all(&emitted.stdout)
        .expect("write artifact");
    let executed = exec.wait_with_output().expect("wait for exec");
    assert!(executed.status.success());
    assert_eq!(executed.stdout, b"42\n");
    assert!(executed.stderr.is_empty());
}

#[test]
fn emit_mir_uses_normal_module_resolution() {
    let output = mojito()
        .args(["emit-mir", "assets/ok/std_traits_origin_imports.mojo"])
        .output()
        .expect("run emit-mir");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.starts_with(b"mojito-mir 1.0\n"));
}

#[test]
fn emit_mir_reports_source_failures_on_stderr() {
    let mut command = mojito();
    command
        .arg("emit-mir")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn emit-mir");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"def main(:\n")
        .expect("write invalid source");
    let output = child.wait_with_output().expect("wait for emit-mir");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("emit-mir error:"));
}
