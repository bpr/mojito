//! The A1 shadow-core proof (`docs/notes/pliron-a1.md`): named checks over
//! the gate fixture `assets/extensions/ok/pliron_a1_gate.mojo`, and over the
//! compile benchmarks for the operations the gate's closure does not hold.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use pliron::context::{Context, Ptr};
use pliron::operation::Operation;
use pliron::r#type::Typed;
use pliron::value::Value;

use mojito::Compiler;
use mojito::backend::pliron::a1;
use mojito::backend::pliron::a1::inventory::{CoreOpKind, Stage};
use mojito::native::mono;

#[path = "pliron_support/mod.rs"]
mod support;

const GATE: &str = "assets/extensions/ok/pliron_a1_gate.mojo";

/// The compile benchmark whose closure holds every operation the gate's
/// does not.
const WIDE: &str = "benchmarks/compile/stdlib_heavy.mojo";

const BENCHMARKS: [&str; 7] = [
    "benchmarks/compile/empty.mojo",
    "benchmarks/compile/hello.mojo",
    "benchmarks/compile/add.mojo",
    "benchmarks/compile/generic.mojo",
    "benchmarks/compile/tuple.mojo",
    "benchmarks/compile/tstring.mojo",
    WIDE,
];

const GATE_STDOUT: &str = "\
9007199254741004 9007199254741005
call
ok
drop part 1
drop proof
after 9007199254741004
finally
end 1
9007199254741004 9007199254741005
call
finally
drop part 2
drop proof
caught
end 2
";

struct Run {
    stdout: String,
    events: Vec<String>,
}

fn run_on_vm(program: mojito::mir::MirProgram) -> Run {
    let mut vm = mojito::backend::VmBackend::new();
    vm.enable_lifecycle_log();
    vm.run_elaborated(program).expect("runs on the VM");
    Run {
        stdout: vm.output(),
        events: vm.lifecycle_log().expect("log enabled").to_vec(),
    }
}

/// The entries the native backend compiles: `main`, and module
/// initialization when the program has any.
fn entries_of(program: &mojito::mir::MirProgram) -> Vec<String> {
    ["main", "__toplevel__"]
        .into_iter()
        .filter(|entry| program.functions.iter().any(|(name, _)| name == entry))
        .map(str::to_string)
        .collect()
}

fn specialized_input(path: &str) -> (mojito::mir::MirProgram, mono::SpecializedProgram) {
    let compiled = Compiler::default()
        .compile_path(Path::new(path))
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    let original = compiled.elaborated_mir().clone();
    let specialized = mono::specialize(&original, &entries_of(&original))
        .unwrap_or_else(|error| panic!("{path}: {}", error.construct));
    (original, specialized)
}

/// What running `program` on the VM gives: its output, or its failure.
fn vm_outcome(program: mojito::mir::MirProgram) -> Result<String, String> {
    let mut vm = mojito::backend::VmBackend::new();
    vm.run_elaborated(program)
        .map(|()| vm.output())
        .map_err(|error| error.to_string())
}

/// Every focused benchmark converts: its specialized closure verifies, the
/// shadow boundary exports its v1 text byte for byte, and the exported
/// program runs as the specialized one does, which runs as the original.
#[test]
fn a1_focused_inputs_convert() {
    let mut coverage = String::from("input\tfunctions\toperations\tv1_bytes\tcore_bytes\n");
    for path in BENCHMARKS {
        let (original, specialized) = specialized_input(path);
        let findings = mojito::mir::verify::verify(&specialized.program);
        assert!(
            findings.is_empty(),
            "{path}: specialized MIR verifies: {findings:?}"
        );
        let refusals = a1::import::refusals(&specialized.program);
        assert!(refusals.is_empty(), "{path}: {refusals:?}");
        let expected_v1 = v1_text(&specialized.program);
        let run = a1::measure::shadow(&original, &entries_of(&original), false)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_eq!(v1_text(&run.exported.program), expected_v1, "{path}");
        let expected = vm_outcome(specialized.program);
        assert_eq!(
            expected,
            vm_outcome(original),
            "{path}: specialized runs as the original"
        );
        assert_eq!(vm_outcome(run.exported.program), expected, "{path}");
        let _ = writeln!(
            coverage,
            "{path}\t{}\t{}\t{}\t{}",
            run.functions,
            run.operations,
            expected_v1.len(),
            run.core_text.len(),
        );
    }
    std::fs::create_dir_all("target/pliron-a1").expect("output directory");
    std::fs::write("target/pliron-a1/benchmark-coverage.tsv", coverage).expect("coverage written");
}

fn specialized_gate() -> (mojito::mir::MirProgram, mono::SpecializedProgram) {
    let src = std::fs::read_to_string(GATE).expect("fixture exists");
    let compiled = Compiler::default()
        .compile_source(&src, Path::new(GATE))
        .expect("gate compiles");
    let original = compiled.elaborated_mir().clone();
    let specialized = mono::specialize(&original, &["main".to_string()]).expect("gate specializes");
    (original, specialized)
}

/// Native specialization is VM-transparent for the gate's closure, and the
/// closure's census is written for the inventory.
#[test]
fn a1_specialized_closure_census() {
    let (original, specialized) = specialized_gate();
    let before = run_on_vm(original);
    assert_eq!(before.stdout, GATE_STDOUT);
    let rows = a1::inventory::census(&specialized.program);
    std::fs::create_dir_all("target/pliron-a1").expect("output directory");
    std::fs::write(
        "target/pliron-a1/op_inventory.tsv",
        a1::inventory::census_tsv(&rows),
    )
    .expect("census written");
    std::fs::write(
        "target/pliron-a1/gate.specialized.mir",
        mojito::mir::text::disassemble(&specialized.program).expect("specialized MIR prints"),
    )
    .expect("specialized MIR written");
    let after = run_on_vm(specialized.program);
    assert_eq!(after.stdout, before.stdout);
    assert_eq!(after.events, before.events);
}

fn import_gate() -> (mono::SpecializedProgram, a1::text::ParsedModule) {
    let (_, specialized) = specialized_gate();
    let mut entries: Vec<(String, String)> = specialized.entries.clone().into_iter().collect();
    entries.sort();
    let mut ctx = a1::ir_framework::new_context();
    let module = a1::import::import_program(&mut ctx, &specialized.program, &entries)
        .unwrap_or_else(|error| panic!("{error}"));
    a1::verify::verify_module(&ctx, module, Stage::Bridge)
        .unwrap_or_else(|error| panic!("{error}"));
    (specialized, a1::text::ParsedModule { ctx, module })
}

fn v1_text(program: &mojito::mir::MirProgram) -> String {
    mojito::mir::text::disassemble(program).expect("MIR prints as v1 text")
}

/// The seven proof items survive MIR -> core -> text -> core -> MIR: both
/// method instances, the exact literal, the reference field, the move, the
/// lifecycle schedule, and both exception paths, with the importer's
/// original program dropped before export.
#[test]
fn a1_representation_contract() {
    let (specialized, mut imported) = import_gate();
    let expected_v1 = v1_text(&specialized.program);
    let expected = run_on_vm(specialized.program);
    assert_eq!(expected.stdout, GATE_STDOUT);

    let text = a1::text::canonical_text(&mut imported.ctx, imported.module, Stage::Bridge)
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write("target/pliron-a1/gate.core.txt", &text).expect("core text written");
    drop(imported);

    for symbol in ["Proof.read$i1;", "Proof.read$i2;"] {
        assert!(text.contains(&format!("mojito_core.signature \"{symbol}\"")));
    }
    assert!(text.contains("IntLiteral(\"9007199254740993\")"));
    assert!(
        !text.contains("mojito-mir"),
        "no MIR text rides in core text"
    );

    let parsed =
        a1::text::parse_text(&text, Stage::Bridge).unwrap_or_else(|error| panic!("{error}"));
    let exported = a1::export::export_program(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let findings = mojito::mir::verify::verify(&exported.program);
    assert!(findings.is_empty(), "exported MIR verifies: {findings:?}");
    assert_eq!(exported.entries, [("main".to_string(), "main".to_string())]);
    let exported_v1 = v1_text(&exported.program);
    std::fs::write("target/pliron-a1/gate.exported.mir", &exported_v1).expect("written");
    assert_eq!(exported_v1, expected_v1, "the bridge preserves v1 text");

    let actual = run_on_vm(exported.program);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.events, expected.events);
}

fn canonical(imported: &mut a1::text::ParsedModule, stage: Stage) -> String {
    a1::text::canonical_text(&mut imported.ctx, imported.module, stage)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn reparse(text: &str, stage: Stage) -> a1::text::ParsedModule {
    a1::text::parse_text(text, stage).unwrap_or_else(|error| panic!("{error}"))
}

/// `C(module) == C(parse(C(module)))` from the very first parse, again
/// after a second parse, and whatever the constructing context allocated
/// before the module.
#[test]
fn a1_canonical_first_print() {
    let (_, mut imported) = import_gate();
    let first = canonical(&mut imported, Stage::Bridge);
    assert!(first.starts_with("mojito-a1-core 0\n"));
    assert!(first.ends_with('\n') && !first.ends_with("\n\n"));
    assert!(!first.contains('\r'));
    assert!(!first.contains("<in-memory>"), "no assembly position leaks");

    let mut once = reparse(&first, Stage::Bridge);
    let second = canonical(&mut once, Stage::Bridge);
    assert_eq!(first, second, "first print is the fixpoint");
    let mut twice = reparse(&second, Stage::Bridge);
    assert_eq!(second, canonical(&mut twice, Stage::Bridge));

    let (_, specialized) = specialized_gate();
    let mut entries: Vec<(String, String)> = specialized.entries.clone().into_iter().collect();
    entries.sort();
    let mut ctx = a1::ir_framework::new_context();
    for index in 0..3 {
        let noise = mojito::Ty::Struct(format!("Noise{index}"), Vec::new());
        a1::types::import_type(&mut ctx, &noise).expect("a nominal type imports");
        if let Err(error) = a1::import::import_program(&mut ctx, &specialized.program, &[]) {
            panic!("{error}");
        }
    }
    let module = a1::import::import_program(&mut ctx, &specialized.program, &entries)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut perturbed = a1::text::ParsedModule { ctx, module };
    assert_eq!(first, canonical(&mut perturbed, Stage::Bridge));
}

/// Every operation keeps its source record, or its recorded absence,
/// across printing, each parse, and export.
#[test]
fn a1_locations_per_op() {
    let (specialized, mut imported) = import_gate();
    let before = a1::provenance::location_map(&imported.ctx, imported.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let text = canonical(&mut imported, Stage::Bridge);
    let stamped = a1::provenance::location_map(&imported.ctx, imported.module)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        stamped
            .iter()
            .all(|(key, record)| record.location.as_ref() == Some(key))
    );
    let provenance = |map: &std::collections::BTreeMap<String, a1::provenance::LocationRecord>| {
        map.iter()
            .map(|(key, record)| (key.clone(), record.provenance.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(provenance(&before), provenance(&stamped));

    let mut once = reparse(&text, Stage::Bridge);
    let after_first = a1::provenance::location_map(&once.ctx, once.module)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        after_first, stamped,
        "the first parse restores every location"
    );
    let again = canonical(&mut once, Stage::Bridge);
    let twice = reparse(&again, Stage::Bridge);
    let after_second = a1::provenance::location_map(&twice.ctx, twice.module)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(after_second, stamped);

    let with_span = stamped
        .values()
        .filter(|record| record.provenance.span.is_some())
        .count();
    let derived = stamped
        .values()
        .filter(|record| !record.provenance.derived_from.is_empty())
        .count();
    let absent = stamped.len() - with_span - derived;
    assert!(with_span > 0 && derived > 0 && absent > 0);
    let labels: std::collections::BTreeSet<&str> = stamped
        .values()
        .filter_map(|record| record.provenance.span.as_ref())
        .filter_map(|span| span.source.as_ref().map(a1::attrs::Text::as_str))
        .collect();
    for label in [
        "assets/extensions/ok/pliron_a1_gate.mojo",
        "assets/extensions/ok/pliron_a1_gate.mojo$Proof$read$i1;",
        "assets/extensions/ok/pliron_a1_gate.mojo$Proof$read$i2;",
    ] {
        assert!(labels.contains(label), "{label} survives exactly");
    }
    std::fs::write(
        "target/pliron-a1/location_counts.tsv",
        format!("source\tderived\tabsent\n{with_span}\t{derived}\t{absent}\n"),
    )
    .expect("counts written");

    let exported = a1::export::export_program(&twice.ctx, twice.module)
        .unwrap_or_else(|error| panic!("{error}"));
    for ((name, function), (expected_name, expected)) in exported
        .program
        .functions
        .iter()
        .zip(&specialized.program.functions)
    {
        assert_eq!(name, expected_name);
        assert_eq!(
            function.spans.0, expected.spans.0,
            "{name}: every record, syntax ids included"
        );
    }
}

type LocationMap = std::collections::BTreeMap<String, a1::provenance::LocationRecord>;

/// A change to one register's source record.
type RecordMutation = fn(&mut (mojito::SourceSpan, Option<u32>));

/// A change to a module, applied before it is verified.
type Mutation = fn(&mut Context, Ptr<Operation>);

fn changed_keys(left: &LocationMap, right: &LocationMap) -> Vec<String> {
    left.iter()
        .filter(|(key, record)| right.get(*key) != Some(record))
        .map(|(key, _)| key.clone())
        .chain(right.keys().filter(|key| !left.contains_key(*key)).cloned())
        .collect()
}

fn round_trip_locations(
    program: &mojito::mir::MirProgram,
) -> (LocationMap, mojito::mir::MirProgram) {
    let mut ctx = a1::ir_framework::new_context();
    let module = a1::import::import_program(&mut ctx, program, &[])
        .unwrap_or_else(|error| panic!("{error}"));
    let mut imported = a1::text::ParsedModule { ctx, module };
    let text = canonical(&mut imported, Stage::Bridge);
    let parsed = reparse(&text, Stage::Bridge);
    let map = a1::provenance::location_map(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let exported = a1::export::export_program(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    (map, exported.program)
}

/// One changed byte end, source label, origin variable, or absent record
/// changes exactly the operation that carries it, through text and back.
#[test]
fn a1_location_mutations_name_their_op() {
    let (_, specialized) = specialized_gate();
    let (baseline, _) = round_trip_locations(&specialized.program);
    let risky = specialized
        .program
        .functions
        .iter()
        .position(|(name, _)| name == "risky")
        .expect("risky is in the closure");
    let key = "risky||0|0|Primary".to_string();
    let mutations: [(&str, RecordMutation); 4] = [
        ("byte end", |record| record.0.span.1 += 1),
        ("source label", |record| {
            record.0.source = Some("répertoire/π \"quoted\" \\.mojo".to_string());
        }),
        ("origin variable", |record| record.1 = Some(0)),
        ("other file, same offsets", |record| {
            record.0.source = Some("assets/extensions/ok/other.mojo".to_string());
        }),
    ];
    for (what, mutate) in mutations {
        let mut program = specialized.program.clone();
        let record = program.functions[risky]
            .1
            .spans
            .0
            .get_mut(&0)
            .expect("%r0 is located");
        mutate(record);
        let expected = program.functions[risky].1.spans.0.clone();
        let (map, exported) = round_trip_locations(&program);
        assert_eq!(
            changed_keys(&baseline, &map),
            std::slice::from_ref(&key),
            "{what}"
        );
        assert_eq!(exported.functions[risky].1.spans.0, expected, "{what}");
    }

    let mut program = specialized.program.clone();
    program.functions[risky].1.spans.0.remove(&0);
    let (map, exported) = round_trip_locations(&program);
    assert_eq!(
        changed_keys(&baseline, &map),
        std::slice::from_ref(&key),
        "absent record"
    );
    assert!(map[&key].provenance.span.is_none());
    assert!(!exported.functions[risky].1.spans.0.contains_key(&0));

    let mut ctx = a1::ir_framework::new_context();
    let module = a1::import::import_program(&mut ctx, &specialized.program, &[])
        .unwrap_or_else(|error| panic!("{error}"));
    let derived = a1::verify::walk(&ctx, module)
        .into_iter()
        .find(|op| {
            a1::verify::attr::<a1::attrs::ProvenanceAttr>(&ctx, *op, &a1::ops::KEY_PROVENANCE)
                .is_some_and(|provenance| !provenance.derived_from.is_empty())
        })
        .expect("the gate has derived operations");
    let identity: a1::attrs::IdentityAttr =
        a1::verify::attr(&ctx, derived, &a1::ops::KEY_IDENTITY).expect("identity");
    let mut provenance: a1::attrs::ProvenanceAttr =
        a1::verify::attr(&ctx, derived, &a1::ops::KEY_PROVENANCE).expect("provenance");
    provenance.derived_from[0] = "elsewhere".into();
    a1::import::set(&ctx, derived, &a1::ops::KEY_PROVENANCE, provenance);
    let mut mutated = a1::text::ParsedModule { ctx, module };
    let text = canonical(&mut mutated, Stage::Bridge);
    let parsed = reparse(&text, Stage::Bridge);
    let map = a1::provenance::location_map(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        changed_keys(&baseline, &map),
        [identity.key()],
        "derivation key"
    );
}

/// The proof tool's v1 output runs on the production `exec` verb, which
/// stays on v1 and never reads core text.
#[test]
fn a1_v1_exec_roundtrip() {
    let (_, mut imported) = import_gate();
    let text = canonical(&mut imported, Stage::Bridge);
    drop(imported);
    let parsed = reparse(&text, Stage::Bridge);
    let exported = a1::export::export_program(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("gate.mir");
    std::fs::write(&artifact, v1_text(&exported.program)).expect("artifact written");
    let exec = |path: &Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_mojito"))
            .arg("exec")
            .arg(path)
            .output()
            .expect("mojito exec runs")
    };
    let run = exec(&artifact);
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), GATE_STDOUT);

    let core = dir.path().join("gate.core");
    std::fs::write(&core, &text).expect("core text written");
    let refused = exec(&core);
    assert_ne!(
        refused.status.code(),
        Some(0),
        "exec does not consume core text"
    );
}

fn normalized_gate() -> (mono::SpecializedProgram, a1::text::ParsedModule) {
    let (specialized, mut imported) = import_gate();
    a1::outcomes::normalize(&mut imported.ctx, imported.module)
        .unwrap_or_else(|error| panic!("{error}"));
    a1::verify::verify_module(&imported.ctx, imported.module, Stage::ExecutableCore)
        .unwrap_or_else(|error| panic!("{error}"));
    (specialized, imported)
}

fn export_executable(parsed: &mut a1::text::ParsedModule) -> a1::export::Exported {
    a1::outcomes::denormalize(&mut parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    a1::verify::verify_module(&parsed.ctx, parsed.module, Stage::Bridge)
        .unwrap_or_else(|error| panic!("{error}"));
    let exported = a1::export::export_program(&parsed.ctx, parsed.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let findings = mojito::mir::verify::verify(&exported.program);
    assert!(findings.is_empty(), "exported MIR verifies: {findings:?}");
    exported
}

/// Executable core holds no bridge operation, and v1 is rebuilt from its
/// normalized operations after the importer's program is gone.
#[test]
fn a1_normalized_outcomes_round_trip() {
    let (specialized, mut normalized) = normalized_gate();
    let expected_v1 = v1_text(&specialized.program);
    let expected = run_on_vm(specialized.program);
    let text = canonical(&mut normalized, Stage::ExecutableCore);
    std::fs::write("target/pliron-a1/gate.executable.txt", &text).expect("written");
    drop(normalized);
    for bridge in ["mojito_core.try_bridge", "mojito_core.region_exit"] {
        assert!(!text.contains(bridge), "{bridge} survives normalization");
    }
    for executable in [
        "mojito_core.invoke",
        "mojito_core.outcome",
        "mojito_core.resume",
    ] {
        assert!(text.contains(executable), "{executable} is exercised");
    }
    let mut parsed = reparse(&text, Stage::ExecutableCore);
    assert_eq!(text, canonical(&mut parsed, Stage::ExecutableCore));
    let exported = export_executable(&mut parsed);
    assert_eq!(v1_text(&exported.program), expected_v1);
    let actual = run_on_vm(exported.program);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.events, expected.events);
}

fn import_bridge(specialized: &mono::SpecializedProgram) -> a1::text::ParsedModule {
    let mut entries: Vec<(String, String)> = specialized.entries.clone().into_iter().collect();
    entries.sort();
    let mut ctx = a1::ir_framework::new_context();
    let module = a1::import::import_program(&mut ctx, &specialized.program, &entries)
        .unwrap_or_else(|error| panic!("{error}"));
    a1::text::ParsedModule { ctx, module }
}

fn import_at(specialized: &mono::SpecializedProgram, stage: Stage) -> a1::text::ParsedModule {
    match stage {
        Stage::Bridge => import_bridge(specialized),
        Stage::ExecutableCore => import_executable(specialized),
    }
}

/// The closures that give every registry entry a positive case: the gate,
/// then the benchmark holding the operations the gate's closure lacks.
fn positive_sources() -> [mono::SpecializedProgram; 2] {
    [specialized_gate().1, specialized_input(WIDE).1]
}

/// The operations `specialized` holds at `stage`.
fn kinds_at(
    specialized: &mono::SpecializedProgram,
    stage: Stage,
) -> std::collections::BTreeSet<CoreOpKind> {
    let module = import_at(specialized, stage);
    a1::verify::walk(&module.ctx, module.module)
        .into_iter()
        .skip(1)
        .map(|op| CoreOpKind::of(&module.ctx, op).expect("a registered operation"))
        .collect()
}

fn import_executable(specialized: &mono::SpecializedProgram) -> a1::text::ParsedModule {
    let mut imported = import_bridge(specialized);
    a1::outcomes::normalize(&mut imported.ctx, imported.module)
        .unwrap_or_else(|error| panic!("{error}"));
    imported
}

fn ops_of_kind(ctx: &Context, module: Ptr<Operation>, kind: CoreOpKind) -> Vec<Ptr<Operation>> {
    a1::verify::walk(ctx, module)
        .into_iter()
        .filter(|op| CoreOpKind::of(ctx, *op) == Some(kind))
        .collect()
}

fn key_of(ctx: &Context, op: Ptr<Operation>) -> String {
    a1::verify::attr::<a1::attrs::IdentityAttr>(ctx, op, &a1::ops::KEY_IDENTITY)
        .expect("identity")
        .key()
}

fn op_with_key(ctx: &Context, module: Ptr<Operation>, key: &str) -> Ptr<Operation> {
    a1::verify::walk(ctx, module)
        .into_iter()
        .find(|op| {
            a1::verify::attr::<a1::attrs::IdentityAttr>(ctx, *op, &a1::ops::KEY_IDENTITY)
                .is_some_and(|identity| identity.key() == key)
        })
        .unwrap_or_else(|| panic!("no operation is `{key}`"))
}

/// The function holding `op`, and a slot of it to use as a stray operand.
fn enclosing(ctx: &Context, op: Ptr<Operation>) -> (Ptr<Operation>, Value) {
    let mut func = op;
    while CoreOpKind::of(ctx, func) != Some(CoreOpKind::Func) {
        func = func
            .deref(ctx)
            .get_parent_op(ctx)
            .expect("inside a function");
    }
    let slot = ops_of_kind(ctx, func, CoreOpKind::Slot)
        .first()
        .map(|slot| slot.deref(ctx).get_result(0))
        .expect("the function has a slot");
    (func, slot)
}

/// How one mutated module fared.
enum Verdict {
    Rejected(String),
    Accepted,
    Panicked,
}

fn verdict(
    module: a1::text::ParsedModule,
    stage: Stage,
    mutate: impl FnOnce(&mut Context, Ptr<Operation>),
) -> Verdict {
    let outcome = catch_unwind(AssertUnwindSafe(move || {
        let mut module = module;
        mutate(&mut module.ctx, module.module);
        a1::verify::verify_module(&module.ctx, module.module, stage)
    }));
    match outcome {
        Ok(Ok(())) => Verdict::Accepted,
        Ok(Err(error)) => Verdict::Rejected(error.to_string()),
        Err(_) => Verdict::Panicked,
    }
}

/// Every registered operation has a verified positive case, and its
/// arity, type, symbol, and control-flow mutations are diagnostics: none
/// is accepted and none panics.
#[test]
fn a1_malformed_ops_are_diagnostics() {
    let sources = positive_sources();
    let mut present = Vec::new();
    for specialized in &sources {
        for stage in [Stage::Bridge, Stage::ExecutableCore] {
            let module = import_at(specialized, stage);
            a1::verify::verify_module(&module.ctx, module.module, stage)
                .unwrap_or_else(|error| panic!("{stage:?}: {error}"));
            present.push((specialized, stage, kinds_at(specialized, stage)));
        }
    }
    let mutations: [(&str, Mutation); 5] = [
        ("identity", |ctx, op| {
            op.deref_mut(ctx)
                .attributes
                .0
                .remove(&*a1::ops::KEY_IDENTITY);
        }),
        ("arity", |ctx, op| {
            if CoreOpKind::of(ctx, op) == Some(CoreOpKind::Func) {
                let ty = a1::types::IntType::get(ctx).into();
                Operation::push_result(op, ctx, ty);
            } else {
                let (_, stray) = enclosing(ctx, op);
                Operation::push_operand(op, ctx, stray);
            }
        }),
        ("type", |ctx, op| {
            let effect = a1::types::EffectType::get(ctx).into();
            let int = a1::types::IntType::get(ctx).into();
            let first = op.deref(ctx).results().next();
            if let Some(result) = first {
                let changed = if result.get_type(ctx) == effect {
                    int
                } else {
                    effect
                };
                result.set_type(ctx, changed);
            } else if CoreOpKind::of(ctx, op) == Some(CoreOpKind::Func) {
                let entry = op.deref(ctx).get_region(0).deref(ctx).get_entry_block();
                let token = entry.and_then(|entry| entry.deref(ctx).arguments().last());
                token
                    .expect("the entry takes the effect token")
                    .set_type(ctx, int);
            } else {
                let (_, stray) = enclosing(ctx, op);
                Operation::replace_operand(op, ctx, 0, stray);
            }
        }),
        ("control flow", |ctx, op| {
            if CoreOpKind::of(ctx, op) == Some(CoreOpKind::Func) {
                op.deref_mut(ctx)
                    .attributes
                    .0
                    .remove(&*a1::ops::KEY_SIGNATURE);
            } else {
                let block = op.deref(ctx).get_parent_block().expect("linked");
                Operation::push_successor(op, ctx, block);
            }
        }),
        ("symbol", |ctx, op| {
            let facts = a1::verify::attr::<a1::attrs::CallAttr>(ctx, op, &a1::ops::KEY_CALL);
            match facts {
                Some(mut call) => {
                    call.target = "nowhere".into();
                    call.resolved = None;
                    a1::import::set(ctx, op, &a1::ops::KEY_CALL, call);
                }
                None => {
                    op.deref_mut(ctx)
                        .attributes
                        .0
                        .remove(&*a1::ops::KEY_PROVENANCE);
                }
            }
        }),
    ];
    let mut matrix = String::from("op\tstage\tmutation\tverdict\tdiagnostic\n");
    let (mut accepted, mut panicked, mut cases) = (0, 0, 0);
    for kind in CoreOpKind::ALL {
        let stage = if kind.legal_at(Stage::Bridge) {
            Stage::Bridge
        } else {
            Stage::ExecutableCore
        };
        let source = present
            .iter()
            .find(|(_, at, kinds)| *at == stage && kinds.contains(&kind))
            .map_or_else(
                || panic!("mojito_core.{} has a positive case", kind.name()),
                |(specialized, _, _)| *specialized,
            );
        for (name, mutation) in mutations {
            let outcome = verdict(import_at(source, stage), stage, |ctx, module| {
                let op = ops_of_kind(ctx, module, kind)[0];
                mutation(ctx, op);
            });
            cases += 1;
            let (label, diagnostic) = match outcome {
                Verdict::Rejected(diagnostic) => ("rejected", diagnostic),
                Verdict::Accepted => {
                    accepted += 1;
                    ("ACCEPTED", String::new())
                }
                Verdict::Panicked => {
                    panicked += 1;
                    ("PANICKED", String::new())
                }
            };
            let line = diagnostic.lines().last().unwrap_or_default();
            let _ = writeln!(
                matrix,
                "{}\t{stage:?}\t{name}\t{label}\t{line}",
                kind.name()
            );
        }
    }
    std::fs::write("target/pliron-a1/malformed_matrix.tsv", &matrix).expect("matrix written");
    assert_eq!((accepted, panicked), (0, 0), "of {cases} cases:\n{matrix}");
}

/// Remove an effectful operation without a value result from its chain.
fn unthread(ctx: &mut Context, op: Ptr<Operation>) {
    let token_in = op.deref(ctx).operands().last().expect("an effect operand");
    let token_out = op.deref(ctx).results().last().expect("an effect result");
    token_out.replace_all_uses_with(ctx, &token_in);
    Operation::erase(op, ctx);
}

/// Thread a lifecycle operation over `place` into the chain before `next`.
fn thread_before(
    ctx: &mut Context,
    kind: CoreOpKind,
    lifecycle: a1::attrs::LifecycleAttr,
    like: Ptr<Operation>,
    place: Value,
    next: Ptr<Operation>,
) {
    let effect = a1::types::EffectType::get(ctx).into();
    let position = next.deref(ctx).get_num_operands() - 1;
    let token = next.deref(ctx).get_operand(position);
    let op = a1::ops::build(ctx, kind, vec![effect], vec![place, token], vec![], 0);
    let attributes = like.deref(ctx).attributes.clone();
    op.deref_mut(ctx).attributes = attributes;
    a1::import::set(ctx, op, &a1::ops::KEY_LIFECYCLE, lifecycle);
    op.insert_before(ctx, next);
    let chained = op.deref(ctx).get_result(0);
    Operation::replace_operand(next, ctx, position, chained);
}

/// The imported lifecycle schedule is checked, not trusted: each mutation
/// of an event, an edge, or an owner fails verification naming the event.
#[test]
fn a1_lifecycle_edges() {
    let (_, specialized) = specialized_gate();
    let sound = import_executable(&specialized);
    a1::verify::verify_module(&sound.ctx, sound.module, Stage::ExecutableCore)
        .unwrap_or_else(|error| panic!("{error}"));
    let exercise = a1::outcomes::functions(&sound.ctx, sound.module)
        .into_iter()
        .find(|func| key_of(&sound.ctx, *func).starts_with("exercise|"))
        .expect("exercise is in the closure");
    let contract: a1::attrs::ContractAttr =
        a1::verify::attr(&sound.ctx, exercise, &a1::ops::KEY_CONTRACT).expect("contract");
    let guarded = contract.0.iter().filter(|event| {
        event.kind == a1::attrs::EventKind::Drop
            && event.before & (a1::lifecycle::ENDED | a1::lifecycle::MOVED) != 0
    });
    assert!(
        guarded.count() > 0,
        "deliberate cleanup of an already-empty slot stays legal"
    );
    drop(sound);

    let inner_drop = "exercise|try0.body.try0.body|0|5|Primary";
    let mutations: [(&str, &str, Mutation); 7] = [
        (
            "swap sibling drops",
            "exercise||0|5|Primary",
            |ctx, module| {
                let first = op_with_key(ctx, module, "exercise||0|5|Primary");
                let second = op_with_key(ctx, module, "exercise||0|6|Primary");
                let (first_place, second_place) = (
                    first.deref(ctx).get_operand(0),
                    second.deref(ctx).get_operand(0),
                );
                Operation::replace_operand(first, ctx, 0, second_place);
                Operation::replace_operand(second, ctx, 0, first_place);
                let first_attributes = first.deref(ctx).attributes.clone();
                let second_attributes = second.deref(ctx).attributes.clone();
                first.deref_mut(ctx).attributes = second_attributes;
                second.deref_mut(ctx).attributes = first_attributes;
            },
        ),
        (
            "omit an error cleanup",
            "exercise|try0|0|0|",
            |ctx, module| {
                let cleanup = op_with_key(ctx, module, "exercise|try0|0|0|UnwindDrop(0)");
                unthread(ctx, cleanup);
            },
        ),
        (
            "bypass finally",
            "exercise|try0.body.try0|0|0|UnwindExit",
            |ctx, module| {
                let exit = op_with_key(ctx, module, "exercise|try0.body.try0|0|0|UnwindExit");
                let outer = op_with_key(ctx, module, "exercise|try0|0|0|UnwindDrop(0)")
                    .deref(ctx)
                    .get_parent_block()
                    .expect("linked");
                Operation::replace_successor(exit, ctx, 0, outer);
            },
        ),
        (
            "drop the moved-from owner as live",
            "exercise||0|6|Primary",
            |ctx, module| {
                let moved = op_with_key(ctx, module, "exercise|try0.body|0|5|Primary");
                a1::import::set(
                    ctx,
                    moved,
                    &a1::ops::KEY_USE_MODE,
                    a1::attrs::UseModeAttr::Copy,
                );
            },
        ),
        (
            "duplicate an unguarded live drop",
            "exercise|try0.body.try0.body|0|5|Primary",
            |ctx, module| {
                let live = op_with_key(ctx, module, "exercise|try0.body.try0.body|0|5|Primary");
                let lifecycle =
                    a1::verify::attr(ctx, live, &a1::ops::KEY_LIFECYCLE).expect("lifecycle");
                let place = live.deref(ctx).get_operand(0);
                thread_before(ctx, CoreOpKind::Drop, lifecycle, live, place, live);
            },
        ),
        (
            "change a projection",
            "Proof.read$i1;||0|1|Place(0)",
            |ctx, module| {
                let place = op_with_key(ctx, module, "Proof.read$i1;||0|1|Place(0)");
                let mut projection: a1::attrs::ProjectionAttr =
                    a1::verify::attr(ctx, place, &a1::ops::KEY_PROJECTION).expect("projection");
                projection.steps[0].kind = a1::attrs::CoreStepKind::Field("exact".into());
                a1::import::set(ctx, place, &a1::ops::KEY_PROJECTION, projection);
            },
        ),
        (
            "consume before a raising call's success edge",
            "exercise|try0|0|0|UnwindDrop(0)",
            |ctx, module| {
                let live = op_with_key(ctx, module, "exercise|try0.body.try0.body|0|5|Primary");
                let invoke = op_with_key(ctx, module, "exercise|try0.body.try0.body|0|1|Primary");
                assert_eq!(CoreOpKind::of(ctx, invoke), Some(CoreOpKind::Invoke));
                let place = live.deref(ctx).get_operand(0);
                let lifecycle = a1::attrs::LifecycleAttr {
                    kind: a1::attrs::CoreLifecycle::ConsumeVar,
                    ..a1::verify::attr(ctx, live, &a1::ops::KEY_LIFECYCLE).expect("lifecycle")
                };
                thread_before(ctx, CoreOpKind::Consume, lifecycle, live, place, invoke);
                unthread(ctx, live);
            },
        ),
    ];
    assert!(mutations.iter().any(|(_, key, _)| *key == inner_drop));
    for (what, names, mutation) in mutations {
        match verdict(
            import_executable(&specialized),
            Stage::ExecutableCore,
            mutation,
        ) {
            Verdict::Rejected(diagnostic) => {
                assert!(diagnostic.contains(names), "{what}: {diagnostic}");
            }
            Verdict::Accepted => panic!("{what}: accepted"),
            Verdict::Panicked => panic!("{what}: panicked"),
        }
    }
}

/// The registry is the dialect: every registered operation is a registry
/// entry with every decision made, and every MIR form is imported by one
/// operation or rejected by name.
#[test]
fn a1_inventory_is_closed() {
    let source = std::fs::read_to_string("crates/mojito-pliron/src/a1/ops.rs").expect("ops.rs");
    let registered: Vec<&str> = source
        .split("\"mojito_core.")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let registry: Vec<&str> = CoreOpKind::ALL.iter().map(|kind| kind.name()).collect();
    assert_eq!(
        registered, registry,
        "ops.rs registers the registry, in order"
    );
    let definitions = std::fs::read_dir("crates/mojito-pliron/src/a1")
        .expect("a1 sources")
        .map(|entry| std::fs::read_to_string(entry.expect("entry").path()).expect("source"))
        .map(|source| source.matches("#[pliron_op(").count())
        .sum::<usize>();
    assert_eq!(
        definitions, 1,
        "every operation is defined through the registry macro"
    );

    let sources = positive_sources();
    let mut spelled = std::collections::BTreeSet::new();
    let modules = sources.iter().flat_map(|specialized| {
        [Stage::Bridge, Stage::ExecutableCore].map(|stage| (stage, import_at(specialized, stage)))
    });
    for (stage, mut module) in modules {
        let present: std::collections::BTreeSet<CoreOpKind> =
            a1::verify::walk(&module.ctx, module.module)
                .into_iter()
                .skip(1)
                .map(|op| CoreOpKind::of(&module.ctx, op).expect("a registered operation"))
                .collect();
        let text = canonical(&mut module, stage);
        for kind in &present {
            assert!(kind.legal_at(stage));
            let spelling = format!("mojito_core.{} (", kind.name());
            assert!(text.contains(&spelling), "{spelling} is canonical syntax");
        }
        spelled.extend(present);
    }
    assert_eq!(
        spelled.len(),
        CoreOpKind::ALL.len(),
        "every entry is exercised"
    );

    let mut imported = Vec::new();
    for kind in CoreOpKind::ALL {
        assert!(kind.legal_at(Stage::Bridge) || kind.legal_at(Stage::ExecutableCore));
        assert_eq!(kind.text_rule(), a1::inventory::TextRule::CanonicalSyntax);
        let denormalized = kind.export_rule() == a1::inventory::ExportRule::Denormalized;
        assert_eq!(
            denormalized,
            !kind.legal_at(Stage::Bridge),
            "{}",
            kind.name()
        );
        imported.extend(kind.mir_forms());
    }
    let rejected = a1::inventory::rejected_forms();
    let mut decided: Vec<&str> = imported.iter().chain(&rejected).copied().collect();
    decided.sort_unstable();
    let mut forms: Vec<&str> = mojito::mir::text::INSTRUCTION_MNEMONICS
        .iter()
        .chain(mojito::mir::text::TERMINATOR_MNEMONICS)
        .copied()
        .collect();
    forms.sort_unstable();
    assert_eq!(decided, forms, "each MIR form has exactly one decision");
    std::fs::write(
        "target/pliron-a1/form_decisions.tsv",
        forms.iter().fold(String::new(), |mut out, form| {
            let decision = a1::inventory::importing_op(form).map_or("rejected", CoreOpKind::name);
            let _ = writeln!(out, "{form}\t{decision}");
            out
        }),
    )
    .expect("decisions written");
}

/// Nothing illegal crosses a conversion boundary: a foreign operation, a
/// surviving bridge operation in a nested region, a foreign type, a
/// dangling symbol, a disabled rule, and an unsupported form all fail
/// before emission.
#[test]
fn a1_conversion_is_total() {
    let (_, specialized) = specialized_gate();

    let bridge = import_bridge(&specialized);
    let survivors: Vec<String> =
        a1::verify::legality_violations(&bridge.ctx, bridge.module, Stage::ExecutableCore)
            .into_iter()
            .map(|violation| violation.to_string())
            .filter(|violation| violation.contains("mojito_core.try_bridge"))
            .collect();
    assert_eq!(survivors.len(), 2, "{survivors:?}");
    assert!(survivors[0].contains("exercise||0|4|Primary"));
    assert!(
        survivors[1].contains("exercise|try0.body|0|13|Primary"),
        "the nested try"
    );
    drop(bridge);

    let injections: [(&str, &str, Mutation); 3] = [
        ("a foreign operation", "is not registered", |ctx, module| {
            let anchor = ops_of_kind(ctx, module, CoreOpKind::Return)[0];
            let int = pliron::builtin::types::IntegerType::get(
                ctx,
                64,
                pliron::builtin::types::Signedness::Signless,
            );
            let value = pliron::builtin::attributes::IntegerAttr::new(
                int,
                pliron::utils::apint::APInt::from_i64(
                    7,
                    std::num::NonZeroUsize::new(64).expect("64"),
                ),
            );
            let foreign = pliron::builtin::ops::ConstantOp::new(ctx, Box::new(value));
            pliron::op::Op::get_operation(&foreign).insert_before(ctx, anchor);
        }),
        (
            "an unresolved type",
            "outside the core types",
            |ctx, module| {
                let constant = ops_of_kind(ctx, module, CoreOpKind::Const)[0];
                let int = pliron::builtin::types::IntegerType::get(
                    ctx,
                    64,
                    pliron::builtin::types::Signedness::Signless,
                );
                let result = constant.deref(ctx).get_result(0);
                result.set_type(ctx, int.into());
            },
        ),
        (
            "a dangling symbol",
            "resolves to no function",
            |ctx, module| {
                let call = ops_of_kind(ctx, module, CoreOpKind::Call)[0];
                let mut facts: a1::attrs::CallAttr =
                    a1::verify::attr(ctx, call, &a1::ops::KEY_CALL).expect("call facts");
                facts.target = "vanished".into();
                facts.resolved = None;
                a1::import::set(ctx, call, &a1::ops::KEY_CALL, facts);
            },
        ),
    ];
    for stage in [Stage::Bridge, Stage::ExecutableCore] {
        for (what, names, injection) in injections {
            let module = match stage {
                Stage::Bridge => import_bridge(&specialized),
                Stage::ExecutableCore => import_executable(&specialized),
            };
            let violations = catch_unwind(AssertUnwindSafe(move || {
                let mut module = module;
                injection(&mut module.ctx, module.module);
                a1::verify::legality_violations(&module.ctx, module.module, stage)
            }))
            .unwrap_or_else(|_| panic!("{what}: panicked"));
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.to_string().contains(names)),
                "{what} at {stage:?}: {violations:?}"
            );
        }
    }

    let wide = specialized_input(WIDE).1;
    let gate_kinds = kinds_at(&specialized, Stage::Bridge);
    for kind in CoreOpKind::ALL
        .into_iter()
        .filter(|kind| !kind.mir_forms().is_empty())
    {
        let config = a1::import::ImportConfig {
            disabled: vec![kind],
        };
        let source = if gate_kinds.contains(&kind) {
            &specialized
        } else {
            &wide
        };
        let mut ctx = a1::ir_framework::new_context();
        let refused = a1::import::import_program_with(&mut ctx, &source.program, &[], &config)
            .expect_err("a disabled rule refuses the program");
        assert_eq!(refused.kind, a1::A1ErrorKind::UnsupportedForm);
        assert!(refused.message.contains(kind.name()), "{refused}");
    }

    let mut unsupported = specialized.program;
    unsupported.functions[0].1.blocks[0]
        .instrs
        .push(mojito::mir::MirInstr::Unsupported("probe".to_string()));
    let mut ctx = a1::ir_framework::new_context();
    let refused = a1::import::import_program(&mut ctx, &unsupported, &[])
        .expect_err("an unsupported form refuses the program");
    assert_eq!(refused.kind, a1::A1ErrorKind::UnsupportedForm);
    assert!(refused.message.contains("unsupported"), "{refused}");
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

struct NativeRun {
    stdout: String,
    events: Vec<String>,
}

fn run_natively(
    program: &mojito::mir::MirProgram,
    entry: &str,
    level: mojito::backend::pliron::OptLevel,
    trace_lifecycle: bool,
) -> NativeRun {
    use mojito::backend::pliron as native;
    let src = std::fs::read_to_string(GATE).expect("fixture exists");
    let options = native::CompileOptions {
        entries: vec![entry.to_string()],
        sources: vec![(GATE.to_string(), src)],
        target: support::host_target(),
        trace_lifecycle,
    };
    let mut module = native::compile(program, &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
    a1::execute::native_legality(&module).unwrap_or_else(|error| panic!("{error}"));
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("gate");
    module
        .write_executable(&exe, level, native::DebugInfo::Lines)
        .unwrap_or_else(|error| panic!("exe emission: {error}"));
    let run = std::process::Command::new(&exe)
        .output()
        .expect("executable runs");
    assert_eq!(run.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&run.stderr);
    if !trace_lifecycle {
        assert!(stderr.is_empty(), "{stderr}");
    }
    NativeRun {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        events: stderr
            .lines()
            .filter_map(|line| line.strip_prefix("mjtrace ").map(str::to_string))
            .collect(),
    }
}

/// One parsed module feeds both backends: the original VM run, the
/// specialized VM run, the parsed-core VM run, and native at O0 and
/// release agree on stdout and on the ordered lifecycle log.
#[test]
fn a1_same_module_executes() {
    use mojito::backend::pliron::OptLevel;
    let (original, specialized) = specialized_gate();
    let original = run_on_vm(original);
    assert_eq!(original.stdout, GATE_STDOUT);
    let specialized_run = run_on_vm(specialized.program.clone());
    assert_eq!(specialized_run.stdout, original.stdout);
    assert_eq!(specialized_run.events, original.events);

    let mut normalized = import_executable(&specialized);
    let text = canonical(&mut normalized, Stage::ExecutableCore);
    drop(normalized);
    drop(specialized);
    let exported = a1::execute::program_of_text(&text).unwrap_or_else(|error| panic!("{error}"));
    let entry = exported
        .entries
        .iter()
        .find(|(requested, _)| requested == "main")
        .map(|(_, concrete)| concrete.clone())
        .expect("the module maps its entry");
    std::fs::write(
        "target/pliron-a1/same_module.tsv",
        format!(
            "core_sha256\tv1_sha256\n{}\t{}\n",
            sha256(text.as_bytes()),
            sha256(v1_text(&exported.program).as_bytes())
        ),
    )
    .expect("hashes written");

    let respecialized = mono::specialize(&exported.program, std::slice::from_ref(&entry))
        .expect("the closed program specializes again");
    assert_eq!(
        v1_text(&respecialized.program),
        v1_text(&exported.program),
        "specializing the closed program changes nothing"
    );

    let core = run_on_vm(exported.program.clone());
    assert_eq!(core.stdout, original.stdout);
    assert_eq!(core.events, original.events);
    for level in [OptLevel::O0, OptLevel::Release] {
        let native = run_natively(&exported.program, &entry, level, false);
        assert_eq!(native.stdout, original.stdout, "{level:?}");
    }
    let traced = run_natively(&exported.program, &entry, OptLevel::O0, true);
    assert_eq!(traced.stdout, original.stdout);
    assert_eq!(traced.events, original.events, "ordered lifecycle logs");
}

/// Insert a value-producing operation at the head of `exercise`'s first
/// body segment, as register `reg`.
fn insert_scalar(
    ctx: &mut Context,
    module: Ptr<Operation>,
    kind: CoreOpKind,
    operands: Vec<Value>,
    reg: u32,
    attributes: impl FnOnce(&Context, Ptr<Operation>),
) -> Value {
    let anchor = op_with_key(ctx, module, "exercise||0|0|Primary");
    let int = a1::types::IntType::get(ctx).into();
    let op = a1::ops::build(ctx, kind, vec![int], operands, vec![], 0);
    attributes(ctx, op);
    a1::import::set(ctx, op, &a1::ops::KEY_REG, a1::attrs::RegAttr(reg));
    a1::import::annotate(
        ctx,
        op,
        a1::attrs::IdentityAttr {
            function: "exercise".into(),
            region: "".into(),
            block: 0,
            ordinal: 0,
            role: a1::attrs::CoreRole::Inserted(u64::from(reg)),
        },
        a1::attrs::ProvenanceAttr {
            span: None,
            origin: None,
            derived_from: vec!["exercise||0|0|Primary".into()],
            reason: "dead scalar instrumentation".into(),
        },
    );
    op.insert_before(ctx, anchor);
    op.deref(ctx).get_result(0)
}

/// The gate keeps no dead scalar of its own, so the test inserts an
/// unused constant-plus-add chain and a dead-looking division before the
/// pass runs.
fn instrument(ctx: &mut Context, module: Ptr<Operation>) {
    let exercise = a1::outcomes::functions(ctx, module)
        .into_iter()
        .find(|func| key_of(ctx, *func).starts_with("exercise|"))
        .expect("exercise is in the closure");
    let mut signature: a1::attrs::SignatureAttr =
        a1::verify::attr(ctx, exercise, &a1::ops::KEY_SIGNATURE).expect("signature");
    let first = signature.registers;
    signature.registers += 5;
    a1::import::set(ctx, exercise, &a1::ops::KEY_SIGNATURE, signature);
    let constant = |value: i64| {
        move |ctx: &Context, op: Ptr<Operation>| {
            a1::import::set(
                ctx,
                op,
                &a1::ops::KEY_CONSTANT,
                a1::attrs::ConstAttr::Int(value),
            );
        }
    };
    let infix = |infix: a1::attrs::InfixAttr| {
        move |ctx: &Context, op: Ptr<Operation>| {
            a1::import::set(ctx, op, &a1::ops::KEY_INFIX, infix);
            a1::import::set(
                ctx,
                op,
                &a1::ops::KEY_RESOLVED,
                a1::attrs::ResolvedAttr(None),
            );
        }
    };
    let one = insert_scalar(ctx, module, CoreOpKind::Const, vec![], first, constant(1));
    let two = insert_scalar(
        ctx,
        module,
        CoreOpKind::Const,
        vec![],
        first + 1,
        constant(2),
    );
    insert_scalar(
        ctx,
        module,
        CoreOpKind::Binary,
        vec![one, two],
        first + 2,
        infix(a1::attrs::InfixAttr::Add),
    );
    let nine = insert_scalar(
        ctx,
        module,
        CoreOpKind::Const,
        vec![],
        first + 3,
        constant(9),
    );
    insert_scalar(
        ctx,
        module,
        CoreOpKind::Binary,
        vec![nine, nine],
        first + 4,
        infix(a1::attrs::InfixAttr::FloorDiv),
    );
}

fn operation_count(module: &a1::text::ParsedModule) -> usize {
    a1::verify::walk(&module.ctx, module.module).len()
}

/// Dead scalar elimination deletes real work and nothing the lifecycle
/// schedule, the locations, or the output can see.
#[test]
fn a1_dead_scalar_preserves_events() {
    let (_, specialized) = specialized_gate();
    let expected = run_on_vm(specialized.program.clone());
    let mut module = import_executable(&specialized);
    drop(specialized);
    let contracts = |module: &a1::text::ParsedModule| {
        a1::outcomes::functions(&module.ctx, module.module)
            .into_iter()
            .map(|func| {
                a1::verify::attr::<a1::attrs::ContractAttr>(
                    &module.ctx,
                    func,
                    &a1::ops::KEY_CONTRACT,
                )
                .expect("contract")
            })
            .collect::<Vec<_>>()
    };
    let untouched = a1::opt::eliminate_dead_scalars(&mut module.ctx, module.module)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        untouched.removed.is_empty(),
        "the gate has no natural dead scalar"
    );

    instrument(&mut module.ctx, module.module);
    a1::verify::verify_module(&module.ctx, module.module, Stage::ExecutableCore)
        .unwrap_or_else(|error| panic!("before the pass: {error}"));
    let before_text = canonical(&mut module, Stage::ExecutableCore);
    let before_count = operation_count(&module);
    let before_locations = a1::provenance::location_map(&module.ctx, module.module)
        .unwrap_or_else(|error| panic!("{error}"));
    let before_contracts = contracts(&module);

    let report = a1::opt::eliminate_dead_scalars(&mut module.ctx, module.module)
        .unwrap_or_else(|error| panic!("{error}"));
    a1::verify::verify_module(&module.ctx, module.module, Stage::ExecutableCore)
        .unwrap_or_else(|error| panic!("after the pass: {error}"));
    let after_count = operation_count(&module);
    assert_eq!(report.removed.len(), 3, "{report:?}");
    assert_eq!(before_count - after_count, 3, "a strict reduction");
    assert!(report.removed.iter().all(|key| key.contains("Inserted(")));
    let unused_effects = a1::verify::walk(&module.ctx, module.module)
        .into_iter()
        .filter(|op| {
            CoreOpKind::of(&module.ctx, *op) == Some(CoreOpKind::Call)
                && !op.deref(&module.ctx).get_result(0).is_used(&module.ctx)
        })
        .count();
    assert!(
        unused_effects > 0,
        "an unused call result never makes the call dead"
    );
    assert_eq!(
        contracts(&module),
        before_contracts,
        "the contract is untouched"
    );

    let after_locations = a1::provenance::location_map(&module.ctx, module.module)
        .unwrap_or_else(|error| panic!("{error}"));
    for (key, record) in &after_locations {
        assert_eq!(
            before_locations.get(key),
            Some(record),
            "{key} keeps its record"
        );
    }
    assert_eq!(before_locations.len() - after_locations.len(), 3);

    let text = canonical(&mut module, Stage::ExecutableCore);
    drop(module);
    let mut parsed = reparse(&text, Stage::ExecutableCore);
    assert_eq!(
        text,
        canonical(&mut parsed, Stage::ExecutableCore),
        "first-print fixpoint"
    );
    drop(parsed);
    for survivor in ["FloorDiv", "Int(9)"] {
        assert!(
            text.contains(survivor),
            "{survivor}: a division may trap, so it stays"
        );
    }
    assert!(!text.contains("Int(2)"));
    std::fs::write(
        "target/pliron-a1/dce.tsv",
        format!(
            "ops_before\tops_after\tbytes_before\tbytes_after\tsweeps\n{before_count}\t{after_count}\t{}\t{}\t{}\n",
            before_text.len(),
            text.len(),
            report.sweeps
        ),
    )
    .expect("measurements written");

    let exported = a1::execute::program_of_text(&text).unwrap_or_else(|error| panic!("{error}"));
    let exercise = exported
        .program
        .functions
        .iter()
        .find(|(name, _)| name == "exercise")
        .map(|(_, function)| function)
        .expect("exercise is exported");
    let first = exercise.n_regs - 5;
    for removed in first..first + 3 {
        assert!(
            !exercise.reg_types.contains_key(&removed),
            "%r{removed} left the tables"
        );
    }
    for kept in [first + 3, first + 4] {
        assert_eq!(exercise.reg_types.get(&kept), Some(&mojito::Ty::Int));
    }
    let core = run_on_vm(exported.program.clone());
    assert_eq!(core.stdout, expected.stdout);
    assert_eq!(core.events, expected.events);
    let traced = run_natively(
        &exported.program,
        "main",
        mojito::backend::pliron::OptLevel::O0,
        true,
    );
    assert_eq!(traced.stdout, expected.stdout);
    assert_eq!(traced.events, expected.events);
}

use a1::params::{
    NodeKey, ParamExprAttr, PayloadNode, PayloadTy, clone_param, export_param, import_param,
    is_executable,
};
use mojito::ast::InfixOp;
use mojito::param_expr::{
    HoleKind, MetaTy, ParamBindings, ParamContext, ParamError, ParamExpr, ParamId,
};
use mojito::types::{TransferEffect, TransferSet};
use mojito::{CtValue, IntLiteral, Ty};

fn int_param(params: &ParamContext, owner: &str, slot: usize, name: &str) -> ParamExpr {
    params.register(ParamId::new(owner, slot), name, MetaTy::int())
}

fn literal(params: &ParamContext, value: i64) -> ParamExpr {
    params
        .constant(CtValue::IntLiteral(IntLiteral::from(value)))
        .expect("an integer literal is a constant")
}

fn infix(params: &ParamContext, op: InfixOp, left: &ParamExpr, right: &ParamExpr) -> ParamExpr {
    params
        .infix(op, left, right)
        .expect("well-typed operands build")
}

fn save(ctx: &mut Context, expr: &ParamExpr) -> NodeKey {
    import_param(ctx, expr).unwrap_or_else(|error| panic!("{error}"))
}

fn callable(transfers: TransferSet) -> Ty {
    Ty::Func {
        environment: mojito::origin::CallableEnvironment::default(),
        params: vec![Ty::Int],
        names: vec!["x".to_string()],
        ret: Box::new(Ty::Int),
        required: vec![true],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: vec![None],
        ref_params: Box::new(vec![None]),
        ref_return: None,
        transfers,
    }
}

fn one_transfer() -> TransferSet {
    TransferSet(vec![TransferEffect {
        dest: mojito::origin::SigOrigin::Self_,
        src: mojito::origin::SigOrigin::Param(0),
        src_is_place: true,
        mutable: false,
    }])
}

fn transfers_of(expr: &ParamExpr) -> usize {
    match expr.as_constant() {
        Some(CtValue::Type(ty)) => match &**ty {
            Ty::Func { transfers, .. } => transfers.0.len(),
            other => panic!("a callable type, not {other:?}"),
        },
        other => panic!("a closed type constant, not {other:?}"),
    }
}

/// Construction goes through the front end's constructors, and the
/// payload covers every node kind, holes and queries included.
#[test]
fn a1_param_construction() {
    let params = ParamContext::new();
    let ctx = &mut a1::ir_framework::new_context();
    let n = int_param(&params, "f", 0, "n");
    let sum = infix(&params, InfixOp::Add, &n, &literal(&params, 1));
    let key = save(ctx, &sum);
    assert!(matches!(key.payload(ctx).node, PayloadNode::Op { .. }));
    assert_eq!(export_param(ctx, &params, key).expect("exports"), sum);
    assert!(
        !is_executable(ctx, key),
        "a residual reference is not executable"
    );

    let seven = save(ctx, &literal(&params, 7));
    assert!(is_executable(ctx, seven));
    let hole = save(ctx, &params.hole(HoleKind::Unbound, MetaTy::int()));
    assert!(
        !is_executable(ctx, hole),
        "a hole never enters executable core"
    );

    let t = params.register(ParamId::new("f", 1), "T", MetaTy::Type);
    let nodes = [
        params.index_ref(1, 2, MetaTy::int()),
        params.neg(&n).expect("negation builds"),
        params.identical(&n, &sum),
        params.conforms(&t, "Copyable").expect("conformance builds"),
        params.reflect_query(&t, mojito::param_expr::ReflectQuery::FieldCount),
        params
            .select(vec![Ty::Int, Ty::Bool], &n)
            .expect("selection builds"),
        params.pack_query(
            t.as_decl_ref().expect("a declared parameter"),
            mojito::param_expr::PackQuery::Length,
        ),
    ];
    for node in nodes {
        let key = save(ctx, &node);
        let exported = export_param(ctx, &params, key).expect("exports");
        assert_eq!(exported, node, "{node}");
        assert!(!is_executable(ctx, key), "{node} is symbolic");
    }

    let budget = {
        let mut sum = int_param(&params, "g", 0, "a0");
        for slot in 1..70 {
            let next = int_param(&params, "g", slot, &format!("a{slot}"));
            sum = infix(&params, InfixOp::Add, &sum, &next);
        }
        let squared = infix(&params, InfixOp::Mul, &sum, &sum);
        params.infix(InfixOp::Mul, &squared, &sum)
    };
    assert!(
        matches!(budget, Err(ParamError::Budget { .. })),
        "the expansion budget is the front end's, unchanged"
    );
}

/// Replacement delegates to the front end: the polynomial part folds,
/// an opaque atom does not, and a partial expression keeps its error.
#[test]
fn a1_param_substitution() {
    let params = ParamContext::new();
    let ctx = &mut a1::ir_framework::new_context();
    let n = int_param(&params, "f", 0, "n");
    let sum = infix(&params, InfixOp::Add, &n, &literal(&params, 1));
    let halved = infix(&params, InfixOp::FloorDiv, &n, &literal(&params, 2));
    let (sum_key, halved_key) = (save(ctx, &sum), save(ctx, &halved));

    let mut bindings = ParamBindings::new();
    bindings.bind(ParamId::new("f", 0), literal(&params, 8));
    let replace = |ctx: &mut Context, key: NodeKey| {
        let expr = export_param(ctx, &params, key).expect("exports");
        let replaced = params.replace(&expr, &bindings).expect("replaces");
        (save(ctx, &replaced), replaced)
    };
    let (nine, _) = replace(ctx, sum_key);
    let folded = export_param(ctx, &params, nine).expect("exports");
    let value = folded.as_constant().and_then(|value| match value {
        CtValue::Int(value) => Some(*value),
        CtValue::IntLiteral(value) => value.to_i64(),
        _ => None,
    });
    assert_eq!(value, Some(9), "`n + 1` at 8 is 9");
    assert!(is_executable(ctx, nine));
    let (unfolded, atom) = replace(ctx, halved_key);
    assert!(
        atom.as_constant().is_none(),
        "`8 // 2` stays an atom, not 4"
    );
    assert!(matches!(unfolded.payload(ctx).node, PayloadNode::Op { .. }));
    assert!(!is_executable(ctx, unfolded) || unfolded != nine);
    // The payload keeps the atom, and export rebuilds it unfolded rather
    // than folding it to 4.
    let exported = export_param(ctx, &params, unfolded).expect("exports unfolded");
    assert_eq!(exported, atom);
    assert!(exported.as_constant().is_none());
    assert_eq!(save(ctx, &exported), unfolded);

    let shadowing = int_param(&params, "g", 0, "n");
    let inner = save(
        ctx,
        &infix(&params, InfixOp::Add, &shadowing, &literal(&params, 1)),
    );
    assert_ne!(
        inner, sum_key,
        "another declaration's `n` is another binder"
    );
    assert_eq!(
        replace(ctx, inner).0,
        inner,
        "and `f`'s binding does not reach it"
    );

    let one = params.constant(CtValue::Int(1)).expect("constant");
    let zero = params.constant(CtValue::Int(0)).expect("constant");
    let divided = infix(&params, InfixOp::FloorDiv, &one, &zero);
    let cancelled = infix(&params, InfixOp::Sub, &divided, &divided);
    assert!(
        cancelled.as_constant().is_none(),
        "cancellation keeps the error"
    );
    let key = save(ctx, &cancelled);
    assert!(
        matches!(key.payload(ctx).node, PayloadNode::Op { .. }),
        "and so does the payload"
    );
    // Export keeps the partial atom under the cancelled term too.
    let exported = export_param(ctx, &params, key).expect("exports uncancelled");
    assert_eq!(exported, cancelled);
    assert!(exported.as_constant().is_none());
    let target = &mut a1::ir_framework::new_context();
    let cloned = clone_param(ctx, key, target).expect("a clone needs no constructor");
    assert_eq!(cloned, save(target, &cancelled));
}

/// Identity within a context is the key; the front end's normal form
/// decides it, and occurrence decorations are never shared.
#[test]
fn a1_param_identity() {
    let params = ParamContext::new();
    let ctx = &mut a1::ir_framework::new_context();
    let n = int_param(&params, "f", 0, "n");
    let one = literal(&params, 1);
    let left = save(ctx, &infix(&params, InfixOp::Add, &n, &one));
    let right = save(ctx, &infix(&params, InfixOp::Add, &one, &n));
    assert_eq!(left, right, "`n + 1` is `1 + n`");

    let negated = params.neg(&n).expect("negation builds");
    let twice = params.neg(&negated).expect("negation builds");
    assert_ne!(save(ctx, &twice), save(ctx, &n), "`-(-n)` is not `n`");
    let scaled = infix(&params, InfixOp::Mul, &literal(&params, -1), &n);
    assert_ne!(
        save(ctx, &negated),
        save(ctx, &scaled),
        "`-n` is not `-1 * n`"
    );

    let plain = params.type_shape(callable(TransferSet::default()));
    let decorated = params.type_shape(callable(one_transfer()));
    assert_eq!(plain, decorated, "transfer effects are not type identity");
    let (plain_key, decorated_key) = (save(ctx, &plain), save(ctx, &decorated));
    assert_ne!(
        plain_key, decorated_key,
        "but each occurrence keeps its own"
    );
    let exported = |key| export_param(ctx, &params, key).expect("exports");
    assert_eq!(transfers_of(&exported(plain_key)), 0);
    assert_eq!(transfers_of(&exported(decorated_key)), 1);
    assert_eq!(exported(plain_key), exported(decorated_key));
}

/// A clone into another context is by value: keys never cross, and the
/// structure, decorations included, does.
#[test]
fn a1_param_cross_context_clone() {
    let params = ParamContext::new();
    let source = &mut a1::ir_framework::new_context();
    let n = int_param(&params, "f", 0, "n");
    let t = params.register(ParamId::new("f", 1), "T", MetaTy::Type);
    let proof = Ty::Struct("Proof".to_string(), Vec::new());
    let exprs = [
        infix(&params, InfixOp::Add, &n, &literal(&params, 1)),
        params.conforms(&t, "Copyable").expect("conformance builds"),
        params.type_shape(callable(one_transfer())),
        params.type_shape(proof),
    ];
    let keys: Vec<NodeKey> = exprs.iter().map(|expr| save(source, expr)).collect();

    let target = &mut a1::ir_framework::new_context();
    for noise in 0..5 {
        save(target, &literal(&params, 100 + noise));
    }
    for (key, expr) in keys.iter().zip(&exprs) {
        let cloned = clone_param(source, *key, target).expect("clones");
        let again = clone_param(source, *key, target).expect("clones");
        assert_eq!(cloned, again, "a clone is canonical in its context");
        assert_eq!(cloned, save(target, expr), "and is the node built there");
        let exported = export_param(target, &params, cloned).expect("exports");
        assert_eq!(&exported, expr, "the structure crosses");
    }
    let decorated = clone_param(source, keys[2], target).expect("clones");
    let exported = export_param(target, &params, decorated).expect("exports");
    assert_eq!(transfers_of(&exported), 1, "the decoration crosses");
    let PayloadNode::Constant(a1::params::PayloadValue::Type(PayloadTy::Core(ty))) =
        clone_param(source, keys[3], target)
            .expect("clones")
            .payload(target)
            .node
            .clone()
    else {
        panic!("a closed nominal type constant");
    };
    let local = a1::types::import_type(target, &Ty::Struct("Proof".to_string(), Vec::new()))
        .expect("imports");
    assert_eq!(ty, local, "a cloned type is the target context's own");
}

/// Parameter attributes print and parse as structure, re-keyed into
/// the parsing context, on operations and on nominal types.
#[test]
fn a1_param_canonical_text() {
    let params = ParamContext::new();
    let (_, specialized) = specialized_gate();
    let mut module = import_executable(&specialized);
    let n = int_param(&params, "Buffer", 0, "n");
    let exprs = [
        infix(&params, InfixOp::Add, &n, &literal(&params, 1)),
        params.type_shape(callable(one_transfer())),
        literal(&params, 9_007_199_254_740_993),
        params
            .constant(CtValue::Str("π \"quoted\" \\ line\nbreak".to_string()))
            .expect("constant"),
    ];
    let anchors = ops_of_kind(&module.ctx, module.module, CoreOpKind::Const);
    let key: pliron::identifier::Identifier = "mojito_core_param".try_into().expect("identifier");
    for (expr, anchor) in exprs.iter().zip(&anchors) {
        let node = save(&mut module.ctx, expr);
        a1::import::set(&module.ctx, *anchor, &key, ParamExprAttr(node));
    }
    let text = canonical(&mut module, Stage::ExecutableCore);
    let anchored: Vec<String> = anchors
        .iter()
        .take(exprs.len())
        .map(|anchor| key_of(&module.ctx, *anchor))
        .collect();
    drop(module);
    assert!(text.contains("mojito_core.param_expr"));

    let mut parsed = reparse(&text, Stage::ExecutableCore);
    assert_eq!(text, canonical(&mut parsed, Stage::ExecutableCore));
    for (expr, anchor) in exprs.iter().zip(&anchored) {
        let op = op_with_key(&parsed.ctx, parsed.module, anchor);
        let carried: ParamExprAttr =
            a1::verify::attr(&parsed.ctx, op, &key).expect("the attribute survives");
        let exported = export_param(&parsed.ctx, &params, carried.0).expect("exports");
        assert_eq!(&exported, expr, "{expr}");
        assert_eq!(
            carried.0,
            save(&mut parsed.ctx, expr),
            "re-keyed where it was parsed"
        );
    }

    let instance = Ty::Struct(
        "Buffer".to_string(),
        vec![
            mojito::TyArg::Ty(Ty::Int),
            mojito::TyArg::Val(CtValue::Int(8)),
        ],
    );
    let ctx = &mut a1::ir_framework::new_context();
    let ty = a1::types::import_type(ctx, &instance).expect("a closed instance imports");
    assert_eq!(a1::types::export_type(ctx, ty).expect("exports"), instance);
    let open = Ty::Struct(
        "Buffer".to_string(),
        vec![mojito::TyArg::Val(CtValue::Expr(n))],
    );
    let refused = a1::types::import_type(ctx, &open).expect_err("an open instance is refused");
    assert_eq!(refused.kind, a1::A1ErrorKind::UnsupportedType);
}

/// The summary applies the budgets strictly, at their boundaries, and an
/// uncovered input is never a pass and never a rejection.
#[test]
fn a1_benchmark_accounting() {
    use a1::measure::{AGGREGATE, Sample, Verdict, summarize};
    let samples = |time_ratio: f64, memory_ratio: f64, ok: bool| {
        let mut samples = Vec::new();
        for input in ["one.mojo", "two.mojo"] {
            for run in 0..10 {
                let noise = f64::from(u8::try_from(run).expect("small")).mul_add(0.01, 1.0);
                for (mode, time, memory) in
                    [("baseline", 1.0, 1.0), ("shadow", time_ratio, memory_ratio)]
                {
                    samples.push(Sample {
                        input: input.to_string(),
                        mode: mode.to_string(),
                        run,
                        wall_ns: 1_000_000.0 * noise * time,
                        peak_kib: 50_000.0 * noise * memory,
                        ok: ok || mode == "baseline" || input == "one.mojo",
                    });
                }
            }
        }
        samples
    };
    let verdicts = |time_ratio: f64, memory_ratio: f64| {
        let rows = summarize(&samples(time_ratio, memory_ratio, true), "shadow");
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| (row.time, row.memory) == (rows[0].time, rows[0].memory))
        );
        let aggregate = rows.last().expect("the aggregate row");
        assert_eq!(aggregate.input, AGGREGATE);
        (aggregate.time, aggregate.memory)
    };
    assert_eq!(verdicts(1.19, 1.29), (Verdict::Pass, Verdict::Pass));
    assert_eq!(
        verdicts(1.20, 1.30),
        (Verdict::Inconclusive, Verdict::Inconclusive)
    );
    assert_eq!(verdicts(1.21, 1.31), (Verdict::Fail, Verdict::Fail));
    assert_eq!(verdicts(1.19, 1.31), (Verdict::Pass, Verdict::Fail));

    let noisy: Vec<Sample> = samples(1.15, 1.0, true)
        .into_iter()
        .map(|mut sample| {
            if sample.mode == "shadow" && sample.run % 2 == 0 {
                sample.wall_ns *= 1.06;
            }
            sample
        })
        .collect();
    let rows = summarize(&noisy, "shadow");
    assert_eq!(
        rows[2].time,
        Verdict::Inconclusive,
        "noise across the limit is no pass"
    );
    assert!(rows[2].time_ratio.lower() < 1.20 && rows[2].time_ratio.upper() > 1.20);

    let rows = summarize(&samples(1.0, 1.0, false), "shadow");
    assert_eq!(rows[0].time, Verdict::Pass, "the covered input stands");
    assert_eq!(
        rows[1].time,
        Verdict::Incomplete,
        "the failed input is no pass"
    );
    assert_eq!(
        rows[2].time,
        Verdict::Incomplete,
        "and neither is the aggregate"
    );
    assert_eq!(rows[2].memory, Verdict::Incomplete);
    let unpaired: Vec<Sample> = samples(1.0, 1.0, true)
        .into_iter()
        .filter(|sample| !(sample.mode == "shadow" && sample.run == 9))
        .collect();
    let rows = summarize(&unpaired, "shadow");
    assert!(rows.iter().all(|row| row.time == Verdict::Incomplete));
    assert!(summarize(&[], "shadow")[0].time == Verdict::Incomplete);

    let text = a1::measure::summary_tsv(&summarize(&samples(1.19, 1.29, true), "shadow"));
    assert_eq!(text.lines().count(), 4);
    assert!(text.lines().last().expect("a row").starts_with(AGGREGATE));
}
