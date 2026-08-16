use mojito::mir::text::{disassemble, load_artifact, parse_artifact, verify_artifact};

const MINIMAL: &[u8] = include_bytes!("snapshots/mir/minimal.mir");
const CONTROL_FLOW: &[u8] = include_bytes!("snapshots/mir/control_flow.mir");

#[test]
fn canonical_snapshots_parse_and_reprint() {
    for (name, source) in [("minimal.mir", MINIMAL), ("control_flow.mir", CONTROL_FLOW)] {
        let parsed = parse_artifact(source, name).expect("parse canonical artifact");
        assert_eq!(disassemble(&parsed.program).unwrap().as_bytes(), source);
    }
}

#[test]
fn source_map_locates_functions_blocks_and_instructions() {
    let parsed = parse_artifact(MINIMAL, "minimal.mir").unwrap();
    let instruction = parsed
        .source_map
        .span("function/identity/bb0/instruction/0")
        .expect("instruction span");
    assert_eq!(
        &MINIMAL[instruction.0..instruction.1],
        b"var.use { dest: %r0, var: $v0, mode: copy }"
    );
}

#[test]
fn canonical_snapshots_load_verified() {
    for (name, source) in [("minimal.mir", MINIMAL), ("control_flow.mir", CONTROL_FLOW)] {
        load_artifact(source, name).expect("load verified canonical artifact");
    }
}

#[test]
fn verifier_findings_locate_the_offending_block() {
    let source = String::from_utf8(MINIMAL.to_vec())
        .unwrap()
        .replace("var.use { dest: %r0,", "var.use { dest: %r5,");
    let report = load_artifact(source.as_bytes(), "block.mir").unwrap_err();
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("invalid register r5"))
        .expect("invalid-register finding");
    assert_eq!(diagnostic.context, ["function/identity/bb0"]);
    let located = &source.as_bytes()[diagnostic.span.0..diagnostic.span.1];
    assert!(located.starts_with(b"bb0 {"));
    assert!(located.ends_with(b"}"));
}

#[test]
fn verifier_findings_locate_terminator_blocks() {
    let source = String::from_utf8(MINIMAL.to_vec()).unwrap().replace(
        "terminator: return { value: present(%r0) }",
        "terminator: jump { target: bb7 }",
    );
    let report = load_artifact(source.as_bytes(), "terminator.mir").unwrap_err();
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("jump to invalid block 7"))
        .expect("invalid-jump finding");
    assert_eq!(diagnostic.context, ["function/identity/bb0"]);
    let located = &source.as_bytes()[diagnostic.span.0..diagnostic.span.1];
    assert!(located.starts_with(b"bb0 {"));
}

#[test]
fn verify_reports_retain_the_source_name_and_parsed_artifacts_reverify() {
    let source = String::from_utf8(MINIMAL.to_vec())
        .unwrap()
        .replace("var.use { dest: %r0,", "var.use { dest: %r5,");
    let parsed = parse_artifact(source.as_bytes(), "gate.mir").expect("parse still succeeds");
    let report = verify_artifact(&parsed, "gate.mir").unwrap_err();
    assert_eq!(report.source_name, "gate.mir");
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn comments_whitespace_and_trailing_commas_are_accepted() {
    let source = b"mojito-mir 1.0\n# artifact comment\nartifact { features: [], files: [], structs: [], decls: [], functions: [], } # eof";
    let parsed = parse_artifact(source, "comments.mir").expect("parse comments");
    assert!(parsed.program.functions.is_empty());
}

#[test]
fn invalid_utf8_and_bom_have_precise_diagnostics() {
    let utf8 = parse_artifact(b"mojito-mir 1.0\n\xff", "utf8.mir").unwrap_err();
    assert_eq!(utf8.diagnostics[0].span, (15, 16));
    assert!(utf8.diagnostics[0].message.contains("UTF-8"));

    let bom = parse_artifact(b"\xef\xbb\xbfmojito-mir 1.0", "bom.mir").unwrap_err();
    assert_eq!(bom.diagnostics[0].span, (0, 3));
}

#[test]
fn syntax_and_structural_errors_are_source_located() {
    let bad_header = parse_artifact(b"other 1.0 artifact {}", "header.mir").unwrap_err();
    assert_eq!(bad_header.diagnostics[0].span, (0, 5));

    let bad_field = b"mojito-mir 1.0 artifact { features: [], files: [], structs: [], decls: [], functions: [], surprise: true }";
    let report = parse_artifact(bad_field, "field.mir").unwrap_err();
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("unknown field"))
        .expect("unknown-field diagnostic");
    assert_eq!(
        &bad_field[diagnostic.span.0..diagnostic.span.1],
        b"surprise"
    );
}

#[test]
fn artifact_report_retains_the_diagnostic_source_name() {
    let report = parse_artifact(b"", "broken.mir").unwrap_err();
    assert_eq!(report.source_name, "broken.mir");
    assert!(report.to_string().contains("broken.mir"));
}
