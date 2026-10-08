//! Per-fixture corpus tests over `assets/` — one libtest-mimic trial per
//! `.mojo` file, so nextest (and `cargo test`'s thread pool) schedule fixture
//! compiles across cores instead of serializing them inside a single sweep.
//!
//! Each group deliberately pins a distinct pipeline entry path; do not merge
//! their per-fixture work even when they visit the same file:
//!
//! - `assets_<category>::<name>` — outcome classification through the
//!   snippet-mode whole-program `Compiler` (`with_snippet_module_scope`),
//!   asserting the fixture lands at the outcome its folder names (see
//!   `assets/README.md`).
//! - `vm_ok::<name>` — the production `Compiler::default()` compile+execute
//!   path over `assets/ok`.
//! - `erased_vm::<name>` — the production run on concrete MIR against the
//!   erased oracle, over `assets/ok` and `assets/runtime_error`: the same
//!   output or error text and the same ordered lifecycle events, and the
//!   same outcome from the serialized artifact, loaded and elaborated. The
//!   fixtures that do not yet agree are `ERASED_VM_RESIDUE`. The type-error
//!   and ownership-error folders are gated by `assets_*`: elaboration runs
//!   after the checker and cannot accept a program it rejects.
//! - `verify::<category>::<name>` — raw phase functions (link → elaborate →
//!   `check_program` → `lower_checked_program` → `elaborate_drops_program` →
//!   verify) over every executable fixture category.
//! - `roundtrip::<category>::<name>` — canonical-text round trips over the
//!   drop-elaborated MIR of every executable fixture: disassemble → parse →
//!   re-disassemble must reproduce the text byte-for-byte. The second
//!   disassembly re-runs the canonical verifier and schema findings on the
//!   parsed program, so it is also the artifact-side semantic gate.
//! - `origin_ok::<name>` / `origin_error::<name>` — production `Compiler`
//!   accept/reject for checked-origin fixtures.
//! - `ownership_ok::<name>` / `ownership_error::<name>` — the standalone
//!   ownership analysis (parse → elaborate → check → `check_ownership`); every
//!   error fixture must pin its message with `# expect:`.
//!
//! A fixture may pin the reported message with a top `# expect: <substring>`
//! comment, and declare what its semantics need with `# requires: discovery`
//! or `# requires: stdlib` (both valid Mojo — the lexer skips them). Corpus
//! non-emptiness invariants are synthetic `*::guard_*` trials so a violation
//! reads as an ordinary test failure.

use libtest_mimic::{Arguments, Failed, Trial};
use mojito::analysis::elaborate_drops_program;
use mojito::backend::VmBackend;
use mojito::mir::lower_checked_program;
use mojito::mir::text::{disassemble, parse_artifact};
use mojito::mir::verify::verify;
use mojito::{
    Compiler, CompilerError, OwnershipError, check, check_ownership, check_program, elaborate,
    link, parse,
};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    assets_outcome_trials(&mut trials);
    vm_ok_trials(&mut trials);
    erased_vm_trials(&mut trials);
    verify_trials(&mut trials);
    roundtrip_trials(&mut trials);
    origin_trials(&mut trials);
    ownership_trials(&mut trials);
    libtest_mimic::run(&arguments, trials).exit();
}

/// The `.mojo` files in `assets/<category>/`, sorted for deterministic
/// enumeration. A missing corpus directory is a repository defect, reported
/// loudly rather than yielding a silently empty group.
fn fixtures(category: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(category);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("fixture directory {}: {error}", dir.display()))
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "mojo")
        })
        .collect();
    files.sort();
    files
}

/// A category's fixtures across its two folders — the Mojo-compilable ones
/// under `assets/<category>` and the Mojito-extension ones under
/// `assets/extensions/<category>` (`assets/README.md`) — each with the
/// trial-name label of its folder (`ok`, `extensions_ok`).
fn labeled_fixtures(category: &str) -> Vec<(PathBuf, String)> {
    let mut all: Vec<(PathBuf, String)> = fixtures(category)
        .into_iter()
        .map(|path| (path, category.to_string()))
        .collect();
    let extension = format!("extensions/{category}");
    if Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(&extension)
        .is_dir()
    {
        all.extend(
            fixtures(&extension)
                .into_iter()
                .map(|path| (path, format!("extensions_{category}"))),
        );
    }
    all
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .expect("fixture stem")
        .to_string_lossy()
        .into_owned()
}

/// `input()` fixtures are interactive smoke tests. Running one from a test
/// inherits Cargo's stdin and can block indefinitely. (The pliron parity
/// harness runs them instead, piping fixed bytes to both backends.)
fn reads_stdin(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "input.mojo" || name == "pliron_input_echo.mojo")
}

/// The `# expect: <substring>` directive (if any), pinning the error message.
fn expected_substring(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix('#')?
            .trim_start()
            .strip_prefix("expect:")
            .map(|s| s.trim().to_string())
    })
}

/// The `# requires: discovery` directive marks a fixture whose semantics need
/// the `Compiler`'s whole-program discovery/specialization handoff (e.g. the
/// checker-inferred scalar-range constructor rewrite). The phase-composed
/// `verify::*` seam is documented as non-authoritative for exactly that
/// handoff (AGENTS.md), so such fixtures pin lowering and verification
/// through the authoritative `vm_ok`/`assets_ok` Compiler trials instead.
fn requires_discovery(path: &Path) -> bool {
    requires(path, "discovery")
}

/// The `# requires: stdlib` directive marks a fixture that names bundled
/// standard-library types (`StringSpan`, …). The ownership groups enter at
/// raw `parse`, which resolves no module, so such a fixture enters at `link`
/// instead; every other stage of the seam is unchanged.
fn requires_stdlib(path: &Path) -> bool {
    requires(path, "stdlib")
}

/// Whether the fixture carries a `# requires: <value>` directive.
fn requires(path: &Path, value: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|source| {
        source.lines().any(|line| {
            line.trim_start()
                .strip_prefix('#')
                .and_then(|rest| rest.trim_start().strip_prefix("requires:"))
                .is_some_and(|found| found.trim() == value)
        })
    })
}

fn fail(message: String) -> Failed {
    Failed::from(message)
}

/// The pipeline stage at which a program is first rejected (or `Ok`).
#[derive(Debug, PartialEq, Clone, Copy)]
enum Outcome {
    Ok,
    ParseError,
    TypeError,
    OwnershipError,
    RuntimeError,
}

impl Outcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::ParseError => "parse_error",
            Self::TypeError => "type_error",
            Self::OwnershipError => "ownership_error",
            Self::RuntimeError => "runtime_error",
        }
    }
}

/// Run the full pipeline, returning where it first fails and the message.
fn classify(path: &Path) -> (Outcome, String) {
    // Historical fixture files include isolated executable snippets at module
    // scope. Production compilation rejects that non-Mojo convenience; this
    // test opts in explicitly until those fixtures are migrated into `main`.
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = match compiler.compile_path(path) {
        Ok(program) => program,
        Err(CompilerError::Module(mojito::ModuleError::Parse { err, .. })) => {
            return (Outcome::ParseError, err.to_string());
        }
        Err(CompilerError::Parse(error)) => return (Outcome::ParseError, error.to_string()),
        Err(CompilerError::Ownership(error)) => {
            return (Outcome::OwnershipError, error.to_string());
        }
        Err(error) => return (Outcome::TypeError, error.to_string()),
    };
    match compiler.execute(&compiled) {
        Ok(_) => (Outcome::Ok, String::new()),
        // Elaboration is instantiation, which the pin performs at compile
        // time: a program it rejects is rejected before anything runs.
        Err(error @ CompilerError::Elaborate(_)) => (Outcome::TypeError, error.to_string()),
        Err(error) => (Outcome::RuntimeError, error.to_string()),
    }
}

fn assets_outcome_trials(trials: &mut Vec<Trial>) {
    for (category, expected) in [
        ("ok", Outcome::Ok),
        ("parse_error", Outcome::ParseError),
        ("type_error", Outcome::TypeError),
        ("ownership_error", Outcome::OwnershipError),
        ("runtime_error", Outcome::RuntimeError),
    ] {
        for (path, label) in labeled_fixtures(category) {
            if category == "ok" && reads_stdin(&path) {
                continue;
            }
            let name = format!("assets_{label}::{}", stem(&path));
            trials.push(Trial::test(name, move || {
                let source = fs::read_to_string(&path).expect("read fixture");
                let (got, message) = classify(&path);
                let shown = if message.is_empty() {
                    "no error"
                } else {
                    message.as_str()
                };
                if got != expected {
                    return Err(fail(format!(
                        "expected {}, got {} ({shown})",
                        expected.label(),
                        got.label(),
                    )));
                }
                if let Some(substring) = expected_substring(&source)
                    && !message.contains(&substring)
                {
                    return Err(fail(format!(
                        "error did not contain '{substring}' (got: {shown})"
                    )));
                }
                Ok(())
            }));
        }
    }
}

fn vm_ok_trials(trials: &mut Vec<Trial>) {
    let paths: Vec<(PathBuf, String)> = labeled_fixtures("ok")
        .into_iter()
        .filter(|(path, _)| !reads_stdin(path))
        .collect();
    let count = paths.len();
    trials.push(Trial::test("vm_ok::guard_nonempty", move || {
        if count == 0 {
            return Err(fail("expected some ok fixtures".to_string()));
        }
        Ok(())
    }));
    // The VM is the sole executor: every `assets/ok/*.mojo` fixture must pass
    // the authoritative whole-program discovery/specialization pipeline and
    // run without error. (Exact-output correctness is asserted by targeted
    // tests.)
    for (path, label) in paths {
        let name = if label == "ok" {
            format!("vm_ok::{}", stem(&path))
        } else {
            format!("vm_ok::extensions::{}", stem(&path))
        };
        trials.push(Trial::test(name, move || {
            let compiler = Compiler::default();
            let program = compiler
                .compile_path(&path)
                .map_err(|error| fail(format!("compile failed on ok fixture: {error}")))?;
            compiler
                .execute(&program)
                .map_err(|error| fail(format!("vm failed on ok fixture: {error}")))?;
            Ok(())
        }));
    }
}

/// The `erased_vm` trials that do not yet hold, by the `docs/roadmap.md`
/// entry that owns each. A listed trial passes while its fixture still
/// differs and fails once it agrees, so a fix removes its row.
const ERASED_VM_RESIDUE: &[&str] = &[
    // R491, the erased oracle: `ParameterList.get_span` is a static method,
    // whose erased frame has no receiver to read the struct's value pack.
    "value_pack_runtime_read",
    // R315, the erased oracle: a struct's type parameter binds the
    // spelling of its type, which decides no conformance condition.
    "keyed_def_builds_loan_carrying_instance",
    // Section 1, the erased oracle: an erased value carries no type
    // argument to construct `T()` from where `T` is a SIMD type or a struct
    // over value parameters.
    "simd_nullary_construction",
    "simd_parameter_default_construction",
    "tuple_array_defaultable",
    // Section 1, the erased oracle: a bound dispatch spells the
    // requirement's qualifier, which names no lowered overload of the erased
    // receiver whose witness binds the parameter under a name of its own.
    "overloaded_method_own_binder_symbols",
    // Section 1, the erased oracle: an erased value carries no type
    // argument to tell a place pointer bound to `T` from a reference.
    "extensions::subtree_pointer_generic_return_deref",
    "extensions::template_served_iterable_def",
    "generic_field_pointer_deref_in_place",
    "template_served_def_loan_carrying_argument",
    "template_served_loan_carrying_instance",
    // Section 1, the erased oracle: an erased value carries no type
    // argument to spell a type name over a parameter from.
    "generic_param_string_literal_materializes",
    "generic_struct_instance_bodies",
    "generic_struct_instance_dispatch",
    "generic_struct_template_reach",
    "optional_raising_subscript",
    "template_def_string_builtins",
    "template_method_string_builtins",
    // Section 1, the erased oracle: an erased body carries no type
    // argument to take `size_of` of a parameter from.
    "size_of_builtin",
    "template_served_def_closed_call",
    // Section 1, the erased oracle: an erased default function has no
    // value for a compile-time parameter it reads.
    "default_reads_binder",
    // Section 1, the erased oracle: an erased frame carries no type
    // argument to decide a `comptime if` over a type binder with, and runs
    // no thunk for a condition that applies a function.
    "comptime_if_condition_applies_def",
    "comptime_if_condition_reads_index",
    "comptime_if_generic_struct_lifecycle",
    "comptime_if_layout_query",
    "comptime_local_binder_alias",
    "dtype_compile_time_values",
    "dtype_float_query_template_served",
    "lane_template_served",
    "pack_beside_value_binder",
    "simd_symbolic_surface",
    "template_def_converting_argument",
    "template_folded_value",
    "template_two_arms",
    "template_value_shaped_construction",
    "template_value_shaped_operations",
    // R286, the erased oracle: a pack's length is its collector's runtime
    // arity, and an unread `var` collector or a collector-less call has none.
    "pack_length_runtime_position",
    // R295, the erased oracle: an erased frame leaves a pack's reified
    // spellings unbound, so it has no element to construct `Ts[i]()` from
    // (R286 stops a collector-less pack's loop first).
    "pack_element_binding_served",
    // R292, the erased oracle: a kept `comptime for` runs its index as a
    // runtime value, so a vector at the index's width has no known width.
    "comptime_for_index_typed_local",
    // R300, the erased oracle: a vector whose width is a layout query
    // reaches the VM with its slot symbolic.
    "comptime_layout_constant",
    // R301, the erased oracle: a kept `comptime for` bounded by a value
    // parameter or a pack's length has no runtime value to stop at.
    "comptime_for_template_served",
    // R323, the erased oracle: an erased frame carries no type argument to
    // answer a reflection query over a type parameter from.
    "dtype_keyed_overload_beside_keyed",
    "reflection_comptime_for_served",
    "reflection_field_name_materialize",
    "reflection_field_type_construction",
    "reflection_list_materialized_whole",
    "reflection_symbolic_fields",
    "reflection_template_served",
    "template_served_body_call_into_overload_family",
    // R366, the erased oracle: an erased frame runs no thunk for a
    // `comptime for` display over a binder.
    "comptime_display_binding_alias",
    "comptime_display_binding_arguments",
    "comptime_display_binding_reads",
    "comptime_for_aggregate_binder",
    "comptime_for_display_binding",
    "comptime_for_display_element_reads",
    "comptime_for_display_over_binder",
    "comptime_for_string_aggregates",
    "comptime_for_tuple_elements",
    "comptime_for_value_struct_method",
    "pack_def_unkept_loop_template_served",
    // R385, the erased oracle: an erased frame runs no function lifted for
    // an application a type spells.
    "comptime_application_argument",
    "comptime_call_callee_shapes",
    "comptime_call_signature",
    "ctfe_body_requests",
    // R302: concrete MIR never frees the empty entries list a linear
    // `Dict.deinit_with` leaves, where the erased run destroys it.
    "dict_insert_linear_capable",
    // R361, the erased oracle: `Tuple` and `TString` are served by their
    // templates, whose bodies an erased frame cannot always run. A tuple an
    // intrinsic or a named result builds reifies no pack, a pack a
    // forwarding `def` passes on (`__make_tstring`, every t-string) reifies
    // none either, and a default initializer constructs an element from a
    // reified type name.
    "pack_element_default_construction",
    "pack_element_rebind",
    "pack_inferred_through_tuple",
    "repr_type_names",
    "slice_descriptor_protocols",
    "template_tuple_default_initializer",
    "tstring_forms",
    "tstring_generic_interpolation",
    "tstring_lazy",
    "tstring_template_served",
    "tuple_nested_type_arguments",
    "tuple_reverse_concat",
    "type_names_applied_elements",
    "variadic_method_type_params",
    "variadic_pack_upstream_spellings",
    // R407, the erased oracle: a pack reifies its length, not its element
    // types, so `Variant`'s template cannot select an alternative and a
    // static method cannot answer `Self.Ts.contains[T]()`.
    "container_hashable",
    "pack_struct_static_method_through_instance",
    "variadic_pack_forwarding_generic_def",
    "variadic_struct_over_variant_method",
    "variant_deinit_with_linear_payload",
    "variant_duplicate_alternatives",
    "variant_generic_def_operations",
    "variant_hash_dict_key",
    "variant_init_with_constructor",
    "variant_nominal_string_payload",
    "variant_owning_api",
    "variant_string_literal_payload",
    "variant_unsafe_get_static_supported",
];

/// Stdin bytes for the fixtures that call `input()`, so both runs of one
/// read the same line and neither inherits the test runner's stdin.
fn fixture_stdin(path: &Path) -> Option<&'static [u8]> {
    match path.file_name()?.to_str()? {
        "input.mojo" => Some(b"World\n"),
        "pliron_input_echo.mojo" => Some(b"echoed line\n"),
        _ => None,
    }
}

/// What one VM run observed: its output or its failure, and its ordered
/// lifecycle events.
#[derive(Debug, PartialEq, Eq)]
struct VmRun {
    outcome: Result<String, String>,
    lifecycle: Vec<String>,
}

/// Run a program on a fresh VM through `run`, with `stdin` served to
/// `input()`. A lifecycle event names the struct it destroys or consumes;
/// an elaborated instance is reported under its template, as the erased run
/// names it.
fn vm_run(
    stdin: Option<&[u8]>,
    run: impl FnOnce(&mut VmBackend) -> Result<(), mojito::runtime::RuntimeError>,
) -> VmRun {
    catch_unwind(AssertUnwindSafe(|| {
        let mut vm = VmBackend::new();
        vm.enable_lifecycle_log();
        if let Some(bytes) = stdin {
            vm.set_input_override(bytes.to_vec());
        }
        let outcome = run(&mut vm)
            .map(|()| vm.output())
            .map_err(|error| error.to_string());
        let lifecycle = vm
            .lifecycle_log()
            .unwrap_or_default()
            .iter()
            .map(|event| match event.split_once("$mono") {
                Some((template, _))
                    if event.starts_with("drop ") || event.starts_with("consume ") =>
                {
                    template.to_string()
                }
                _ => without_temp_paths(event),
            })
            .collect();
        VmRun { outcome, lifecycle }
    }))
    .unwrap_or_else(|_| VmRun {
        outcome: Err("the VM panicked".to_string()),
        lifecycle: Vec::new(),
    })
}

/// `event` with each path under the temporary directory spelled `<tmp>`: a
/// fixture's temporary files are named at random per run, and a raised
/// error's message may name one.
fn without_temp_paths(event: &str) -> String {
    let temp = std::env::temp_dir();
    let temp = temp.to_string_lossy();
    let temp = temp.trim_end_matches('/');
    let mut out = String::new();
    let mut rest = event;
    while let Some(start) = rest.find(temp) {
        out.push_str(&rest[..start]);
        out.push_str("<tmp>");
        let after = &rest[start + temp.len()..];
        let end = after
            .find(|c: char| c == '\'' || c == '"' || c.is_whitespace())
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Run the fixture as production does, on concrete MIR, and require the
/// erased oracle to observe the same thing: output or error text, and the
/// ordered lifecycle events. The program's serialized artifact, loaded and
/// elaborated, must reach the same outcome.
fn concrete_runs_as_erased(path: &Path) -> Result<(), Failed> {
    let compiled = Compiler::default()
        .compile_path(path)
        .map_err(|error| fail(format!("compile: {error}")))?;
    let concrete = compiled
        .concrete_mir()
        .map_err(|error| fail(format!("elaborate: {error}")))?
        .program
        .clone();
    let stdin = fixture_stdin(path);
    let erased = compiled.drop_elaborated_mir().clone();
    let expected = vm_run(stdin, |vm| vm.run_elaborated(erased));
    let actual = vm_run(stdin, |vm| vm.run_concrete(concrete));
    if actual != expected {
        return Err(fail(format!(
            "concrete program ran to {actual:?}, erased program to {expected:?}"
        )));
    }
    // `run_artifact` reads process stdin, so an `input()` fixture's artifact
    // is covered by the round-trip group alone.
    if stdin.is_none() {
        let text = compiled
            .emit_mir()
            .map_err(|error| fail(format!("emit: {error}")))?;
        let artifact = catch_unwind(AssertUnwindSafe(|| {
            mojito::run_artifact_as(
                text.as_bytes(),
                path.display().to_string(),
                mojito::BackendKind::Vm,
                mojito::VmInstantiation::Concrete,
            )
            .map(|execution| execution.output)
            .map_err(|error| error.to_string())
        }))
        .unwrap_or_else(|_| Err("the VM panicked".to_string()));
        if artifact != actual.outcome {
            return Err(fail(format!(
                "artifact ran to {artifact:?}, source program to {:?}",
                actual.outcome
            )));
        }
    }
    Ok(())
}

fn erased_vm_trials(trials: &mut Vec<Trial>) {
    let paths: Vec<(PathBuf, String)> = ["ok", "runtime_error"]
        .into_iter()
        .flat_map(|category| {
            labeled_fixtures(category)
                .into_iter()
                .map(move |(path, label)| {
                    let name = match (category, label == category) {
                        ("ok", true) => stem(&path),
                        ("ok", false) => format!("extensions::{}", stem(&path)),
                        _ => format!("{label}::{}", stem(&path)),
                    };
                    (path, name)
                })
        })
        .collect();
    let unknown: Vec<&str> = ERASED_VM_RESIDUE
        .iter()
        .copied()
        .filter(|residue| !paths.iter().any(|(_, name)| name == residue))
        .collect();
    let count = paths.len();
    trials.push(Trial::test("erased_vm::guard_corpus", move || {
        if count == 0 {
            return Err(fail("expected some ok fixtures".to_string()));
        }
        if !unknown.is_empty() {
            return Err(fail(format!("residue rows name no fixture: {unknown:?}")));
        }
        Ok(())
    }));
    for (path, name) in paths {
        let residue = ERASED_VM_RESIDUE.contains(&name.as_str());
        trials.push(Trial::test(format!("erased_vm::{name}"), move || {
            match (concrete_runs_as_erased(&path), residue) {
                (Ok(()), true) => Err(fail(
                    "now runs as the erased program: remove its ERASED_VM_RESIDUE row".to_string(),
                )),
                (Err(_), true) => Ok(()),
                (outcome, false) => outcome,
            }
        }));
    }
}

fn verify_trials(trials: &mut Vec<Trial>) {
    let mut total = 0usize;
    for category in ["ok", "origin_ok", "ownership_ok"] {
        for (path, label) in labeled_fixtures(category) {
            if requires_discovery(&path) {
                continue;
            }
            total += 1;
            let name = format!("verify::{label}::{}", stem(&path));
            trials.push(Trial::test(name, move || {
                let program = link(&path).map_err(|error| fail(format!("link: {error}")))?;
                let program =
                    elaborate(program).map_err(|error| fail(format!("elaborate: {error}")))?;
                let checked =
                    check_program(&program).map_err(|error| fail(format!("check: {error:?}")))?;
                let mir = mojito::mir::lower_checked_program(&checked);
                if !mir.invariant_errors.is_empty() {
                    return Err(fail(format!(
                        "invariant errors: {:?}",
                        mir.invariant_errors
                    )));
                }
                // Drop elaboration must preserve every verification invariant
                // the VM relies on: the executed program is the elaborated one.
                let elaborated = elaborate_drops_program(mir);
                let errors = verify(&elaborated);
                if !errors.is_empty() {
                    return Err(fail(format!("{errors:?}")));
                }
                Ok(())
            }));
        }
    }
    trials.push(Trial::test("verify::guard_corpus_size", move || {
        if total <= 40 {
            return Err(fail(format!("fixture corpus unexpectedly small: {total}")));
        }
        Ok(())
    }));
}

/// Canonical executable MIR for a fixture. Discovery-dependent fixtures use
/// the authoritative compiler's cached MIR/artifact seam; raw phase fixtures
/// deliberately retain their independent lowering coverage.
fn roundtrip_text(path: &Path) -> Result<String, Failed> {
    if requires_discovery(path) {
        let compiled = Compiler::default()
            .compile_path(path)
            .map_err(|error| fail(format!("compile: {error}")))?;
        return compiled
            .emit_mir()
            .map_err(|error| fail(format!("emit MIR: {error}")));
    }
    let program = link(path).map_err(|error| fail(format!("link: {error}")))?;
    let program = elaborate(program).map_err(|error| fail(format!("elaborate: {error}")))?;
    let checked = check_program(&program).map_err(|error| fail(format!("check: {error:?}")))?;
    let mir = lower_checked_program(&checked);
    if !mir.invariant_errors.is_empty() {
        return Err(fail(format!(
            "invariant errors: {:?}",
            mir.invariant_errors
        )));
    }
    disassemble(&elaborate_drops_program(mir))
        .map_err(|error| fail(format!("disassemble: {error}")))
}

/// The first line where two disassemblies diverge, for a readable failure.
fn first_divergence(first: &str, second: &str) -> String {
    for (number, (left, right)) in first.lines().zip(second.lines()).enumerate() {
        if left != right {
            return format!(
                "first divergence at line {}:\n  first:  {left}\n  second: {right}",
                number + 1
            );
        }
    }
    format!(
        "one text is a prefix of the other ({} vs {} bytes)",
        first.len(),
        second.len()
    )
}

fn roundtrip_trials(trials: &mut Vec<Trial>) {
    let mut total = 0usize;
    for category in ["ok", "origin_ok", "ownership_ok"] {
        for (path, label) in labeled_fixtures(category) {
            total += 1;
            let name = format!("roundtrip::{label}::{}", stem(&path));
            trials.push(Trial::test(name, move || {
                // A program schema 1.0 cannot represent is exactly the finding
                // this group exists to surface.
                let first = roundtrip_text(&path)?;
                let parsed = parse_artifact(first.as_bytes(), stem(&path)).map_err(|report| {
                    let details: Vec<String> = report
                        .diagnostics
                        .iter()
                        .map(|diagnostic| {
                            format!(
                                "{} at bytes {}..{}: {:?}",
                                diagnostic.message,
                                diagnostic.span.0,
                                diagnostic.span.1,
                                &first[diagnostic.span.0.min(first.len())
                                    ..diagnostic.span.1.min(first.len())]
                            )
                        })
                        .collect();
                    fail(format!("parse: {report}\n{}", details.join("\n")))
                })?;
                // Reprinting re-runs the canonical verifier plus schema
                // findings on the parsed program — the artifact-side gate.
                let second = disassemble(&parsed.program)
                    .map_err(|error| fail(format!("re-disassemble: {error}")))?;
                if first != second {
                    return Err(fail(first_divergence(&first, &second)));
                }
                Ok(())
            }));
        }
    }
    trials.push(Trial::test("roundtrip::guard_corpus_size", move || {
        if total <= 40 {
            return Err(fail(format!("fixture corpus unexpectedly small: {total}")));
        }
        Ok(())
    }));
}

fn origin_trials(trials: &mut Vec<Trial>) {
    for (path, label) in labeled_fixtures("origin_ok") {
        trials.push(Trial::test(
            format!("{label}::{}", stem(&path)),
            move || {
                let compiler = Compiler::default();
                let program = compiler
                    .compile_path(&path)
                    .map_err(|error| fail(error.to_string()))?;
                compiler
                    .execute(&program)
                    .map_err(|error| fail(error.to_string()))?;
                Ok(())
            },
        ));
    }
    for (path, label) in labeled_fixtures("origin_error") {
        let name = format!("{label}::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            let source = fs::read_to_string(&path).expect("read origin fixture");
            let message = match Compiler::default().compile_path(&path) {
                Err(error) => error.to_string(),
                Ok(_) => {
                    return Err(fail(
                        "origin error fixture must fail compilation".to_string(),
                    ));
                }
            };
            if let Some(expected) = expected_substring(&source)
                && !message.contains(&expected)
            {
                return Err(fail(format!("expected '{expected}' in '{message}'")));
            }
            Ok(())
        }));
    }
}

/// Elaborate and type-check a fixture (the production stage order), then run
/// the ownership analysis. (Mirrors the helper `tests/ownership_test.rs` keeps
/// for its targeted tests.) A `# requires: stdlib` fixture enters at `link` so
/// its standard-library types resolve.
fn own(path: &Path) -> Result<(), OwnershipError> {
    let program = if requires_stdlib(path) {
        link(path).expect("link error")
    } else {
        let src = fs::read_to_string(path).expect("read fixture");
        parse(&src).expect("parse error")
    };
    let program = elaborate(program).expect("comptime error");
    check(&program).expect("type error");
    check_ownership(&program)
}

fn ownership_trials(trials: &mut Vec<Trial>) {
    let error_paths = labeled_fixtures("ownership_error");
    let count = error_paths.len();
    trials.push(Trial::test(
        "ownership_error::guard_corpus_size",
        move || {
            if count < 4 {
                return Err(fail(format!(
                    "expected several ownership-error fixtures, found {count}"
                )));
            }
            Ok(())
        },
    ));
    // Every `assets/ownership_error/*.mojo` must be a move violation whose
    // message contains its required `# expect:` substring.
    for (path, label) in error_paths {
        let name = format!("{label}::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            let src = fs::read_to_string(&path).expect("read fixture");
            let expect = expected_substring(&src)
                .expect("ownership_error fixture must pin a `# expect:` substring");
            match own(&path) {
                Err(error) => {
                    let message = error.to_string();
                    if !message.contains(&expect) {
                        return Err(fail(format!(
                            "message {message:?} lacks expected {expect:?}"
                        )));
                    }
                    Ok(())
                }
                Ok(()) => Err(fail("expected an ownership error".to_string())),
            }
        }));
    }
    // Every `assets/ownership_ok/*.mojo` must pass the ownership analysis
    // (analysis only — no VM execution).
    for (path, label) in labeled_fixtures("ownership_ok") {
        let name = format!("{label}::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            own(&path).map_err(|error| fail(format!("expected no ownership error, got {error:?}")))
        }));
    }
}
