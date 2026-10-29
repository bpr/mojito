use super::{instruction_mnemonic, quote, terminator_mnemonic, version_header};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use crate::mir::*;
use mojito_ast::ast::{ArgConvention, InfixOp, PrefixOp};
use mojito_checked::checked::{
    CheckedCallArgument, CheckedCallArgumentSource, CheckedConst, CheckedResultAdapter,
    ComptimeSequence,
};
use mojito_checked::declaration::InlineLevel;
use mojito_types::ct::{CtMarker, CtValue};
use mojito_types::origin::{
    CallableEnvironment, CaptureAccess, CaptureOriginSet, Mutability, Origin, OriginSeg,
    PointerOrigin, RefSig, RefTy, SigMutability, SigOrigin,
};
use mojito_types::param_expr::{MetaTy, PackQuery, ParamExpr, ParamKind};
use mojito_types::types::TyKind;
use mojito_types::types::{
    CallableDefault, ConstraintOperand, DependentType, FuncTy, GenericConstraint, GenericFuncTy,
    PackPredicateRef, ParamDecl, TrivialLifecycle, Ty, TyArg,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub(super) fn schema_findings(program: &MirProgram) -> Vec<String> {
    let mut findings = Vec::new();
    let mut functions = BTreeSet::new();
    for (name, function) in &program.functions {
        if !functions.insert(name) {
            findings.push(format!("duplicate MIR function name `{name}`"));
        }
        if function.var_names.len() != function.n_vars {
            findings.push(format!(
                "function `{name}` has {} variable names for {} slots",
                function.var_names.len(),
                function.n_vars
            ));
        }
        for (reg, (source, _)) in &function.spans.0 {
            if source.span.0 > source.span.1 {
                findings.push(format!(
                    "function `{name}` register %r{reg} has a reversed span"
                ));
            }
        }
    }
    let mut structs = BTreeSet::new();
    for declaration in &program.declarations.structs {
        if !structs.insert(&declaration.name) {
            findings.push(format!(
                "duplicate MIR struct declaration `{}`",
                declaration.name
            ));
        }
    }
    let mut declarations = BTreeSet::new();
    for declaration in &program.declarations.functions {
        if !declarations.insert(&declaration.lowered_name) {
            findings.push(format!(
                "duplicate MIR function declaration `{}`",
                declaration.lowered_name
            ));
        }
    }
    findings
}

pub(super) fn program(program: &MirProgram) -> String {
    let files = files(program);
    let mut output = String::new();
    write_artifact_head(&mut output, program, &files);
    write_functions(&mut output, program, &files);
    writeln!(output, "}}").unwrap();
    output
}

/// The artifact's `functions: [ … ]` section alone, numbered against the
/// whole artifact's file table.
pub(super) fn functions_section(program: &MirProgram) -> String {
    let mut output = String::new();
    write_functions(&mut output, program, &files(program));
    output
}

/// The version header and an artifact's leading fields, through its
/// function declarations.
fn write_artifact_head(
    output: &mut String,
    program: &MirProgram,
    files: &BTreeMap<Option<String>, usize>,
) {
    writeln!(output, "{}", version_header()).unwrap();
    writeln!(output, "artifact {{").unwrap();
    writeln!(output, "  features: [],").unwrap();
    write_files(output, files);
    writeln!(
        output,
        "  traits: {},",
        list(program.declarations.traits.iter().map(|declaration| {
            record(
                "trait",
                &[
                    ("name", symbol(&declaration.name)),
                    (
                        "refines",
                        list(declaration.refines.iter().map(|parent| symbol(parent))),
                    ),
                    (
                        "requirements",
                        list(declaration.requirements.iter().map(|requirement| {
                            record(
                                "requirement",
                                &[
                                    ("name", symbol(&requirement.name)),
                                    ("arity", requirement.arity.to_string()),
                                    ("dispatch", symbol(&requirement.dispatch)),
                                    (
                                        "param_names",
                                        list(requirement.param_names.iter().map(|v| symbol(v))),
                                    ),
                                    (
                                        "param_types",
                                        list(requirement.param_types.iter().map(ty_value)),
                                    ),
                                    (
                                        "param_conventions",
                                        list(
                                            requirement
                                                .param_conventions
                                                .iter()
                                                .map(|v| option(v.map(convention))),
                                        ),
                                    ),
                                    (
                                        "variadic_index",
                                        option(requirement.variadic_index.map(|v| v.to_string())),
                                    ),
                                    (
                                        "required",
                                        list(requirement.required.iter().map(bool::to_string)),
                                    ),
                                    (
                                        "positional_only",
                                        option(requirement.positional_only.map(|v| v.to_string())),
                                    ),
                                    (
                                        "keyword_only",
                                        option(requirement.keyword_only.map(|v| v.to_string())),
                                    ),
                                    (
                                        "variadic",
                                        option(requirement.variadic.as_ref().map(ty_value)),
                                    ),
                                    (
                                        "kw_variadic",
                                        option(requirement.kw_variadic.as_ref().map(ty_value)),
                                    ),
                                    (
                                        "variadic_convention",
                                        option(requirement.variadic_convention.map(convention)),
                                    ),
                                ],
                            )
                        })),
                    ),
                ],
            )
        }))
    )
    .unwrap();
    write_structs(output, &program.declarations.structs);
    write_declarations(output, &program.declarations.functions);
}

fn files(program: &MirProgram) -> BTreeMap<Option<String>, usize> {
    program
        .functions
        .iter()
        .flat_map(|(_, function)| function.spans.0.values())
        .map(|(span, _)| span.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(id, path)| (path, id))
        .collect()
}

fn write_files(output: &mut String, files: &BTreeMap<Option<String>, usize>) {
    writeln!(output, "  files: [").unwrap();
    for (index, (path, id)) in files.iter().enumerate() {
        write!(
            output,
            "    file {{ id: file{id}, path: {}, module: absent }}",
            option(path.as_ref().map(|path| quote(path)))
        )
        .unwrap();
        writeln!(
            output,
            "{}",
            if index + 1 == files.len() { "" } else { "," }
        )
        .unwrap();
    }
    writeln!(output, "  ],").unwrap();
}

fn write_structs(output: &mut String, declarations: &[MirStructDeclaration]) {
    let mut declarations = declarations.iter().collect::<Vec<_>>();
    declarations.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    writeln!(output, "  structs: [").unwrap();
    for (index, declaration) in declarations.iter().enumerate() {
        let fields =
            list(declaration.fields.iter().map(|(name, ty)| {
                record("field", &[("name", symbol(name)), ("type", ty_value(ty))])
            }));
        let mut methods = declaration.mut_self_methods.iter().collect::<Vec<_>>();
        methods.sort();
        let explicit_destroy = declaration.explicit_destroy.as_ref();
        let destructors = {
            let mut entries = declaration.explicit_destructors.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            list(entries.into_iter().map(|(name, raises)| {
                record(
                    "destructor",
                    &[("name", symbol(name)), ("raises", raises.to_string())],
                )
            }))
        };
        let obligated_fields = {
            let mut entries = explicit_destroy
                .into_iter()
                .flat_map(|info| &info.fields)
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            list(entries.into_iter().map(|(name, owner)| {
                record(
                    "field",
                    &[("name", symbol(name)), ("struct", symbol(owner))],
                )
            }))
        };
        let mut entries = vec![
            ("name", symbol(&declaration.name)),
            ("fields", fields),
            (
                "mut_self_methods",
                list(methods.into_iter().map(|name| symbol(name))),
            ),
            ("fieldwise_init", declaration.fieldwise_init.to_string()),
            (
                "param_decls",
                list(declaration.param_decls.iter().map(param_decl)),
            ),
            (
                "explicit_destroy_message",
                option(explicit_destroy.map(|info| quote(&info.message))),
            ),
            ("explicit_destructors", destructors),
            (
                "explicit_destroy_declared",
                explicit_destroy
                    .is_some_and(|info| info.declared)
                    .to_string(),
            ),
            ("explicit_destroy_fields", obligated_fields),
            ("conformances", conformance_rows(&declaration.conformances)),
            ("declared", conformance_rows(&declaration.declared)),
            (
                "associated_types",
                list(declaration.associated_types.iter().map(|(name, ty)| {
                    record(
                        "associated_type",
                        &[("name", symbol(name)), ("type", ty_value(ty))],
                    )
                })),
            ),
            (
                "parameterized_associated_types",
                list(
                    declaration
                        .parameterized_associated_types
                        .iter()
                        .map(parameterized_associated_type),
                ),
            ),
            (
                "associated_values",
                list(declaration.associated_values.iter().map(|(name, value)| {
                    record(
                        "associated_value",
                        &[("name", symbol(name)), ("value", param_expr(value))],
                    )
                })),
            ),
        ];
        // Schema 1.80 carries the parameterized value members, written only
        // when the struct declares one.
        if !declaration.parameterized_associated_values.is_empty() {
            entries.push((
                "parameterized_associated_values",
                list(
                    declaration
                        .parameterized_associated_values
                        .iter()
                        .map(parameterized_associated_value),
                ),
            ));
        }
        // Schema 1.65 carries the member table, written only when the
        // struct owns a lifecycle member.
        if !declaration.members.is_empty() {
            entries.push((
                "members",
                list(declaration.members.iter().map(|entry| {
                    record(
                        "member",
                        &[
                            ("name", symbol(&entry.member)),
                            ("symbol", symbol(&entry.symbol)),
                        ],
                    )
                })),
            ));
        }
        // Schema 1.89 carries the witness tables, written only when the
        // struct declares a conformance with method requirements.
        if !declaration.witness_tables.is_empty() {
            entries.push((
                "witness_tables",
                list(declaration.witness_tables.iter().map(|table| {
                    record(
                        "witness_table",
                        &[
                            ("trait", symbol(&table.trait_name)),
                            (
                                "witnesses",
                                list(table.witnesses.iter().map(|witness| {
                                    record(
                                        "witness",
                                        &[
                                            ("requirement", symbol(&witness.requirement)),
                                            ("member", symbol(&witness.member)),
                                        ],
                                    )
                                })),
                            ),
                        ],
                    )
                })),
            ));
        }
        let value = record("struct", &entries);
        write!(output, "    {value}").unwrap();
        writeln!(
            output,
            "{}",
            if index + 1 == declarations.len() {
                ""
            } else {
                ","
            }
        )
        .unwrap();
    }
    writeln!(output, "  ],").unwrap();
}

/// A struct's conformance rows, each a `conformance` record.
fn conformance_rows(rows: &[mojito_checked::checked::StructConformance]) -> String {
    list(rows.iter().map(|row| {
        record(
            "conformance",
            &[
                ("trait", symbol(&row.trait_name)),
                ("conditions", list(row.conditions.iter().map(constraint))),
            ],
        )
    }))
}

fn write_declarations(output: &mut String, declarations: &[MirFunctionDeclaration]) {
    let mut declarations = declarations.iter().collect::<Vec<_>>();
    declarations.sort_by(|left, right| {
        left.lowered_name
            .as_bytes()
            .cmp(right.lowered_name.as_bytes())
    });
    writeln!(output, "  decls: [").unwrap();
    for (index, declaration) in declarations.iter().enumerate() {
        let value = record(
            "decl",
            &[
                ("lowered_name", symbol(&declaration.lowered_name)),
                (
                    "param_names",
                    list(declaration.param_names.iter().map(|v| symbol(v))),
                ),
                (
                    "param_types",
                    list(declaration.param_types.iter().map(ty_value)),
                ),
                (
                    "defaults",
                    list(
                        declaration
                            .defaults
                            .iter()
                            .map(|v| option(v.as_ref().map(checked_const))),
                    ),
                ),
                (
                    "required",
                    list(declaration.required.iter().map(bool::to_string)),
                ),
                (
                    "variadic",
                    option(declaration.variadic.as_ref().map(ty_value)),
                ),
                (
                    "variadic_convention",
                    option(declaration.variadic_convention.map(convention)),
                ),
                (
                    "variadic_index",
                    option(declaration.variadic_index.map(|v| v.to_string())),
                ),
                (
                    "kw_variadic",
                    option(declaration.kw_variadic.as_ref().map(ty_value)),
                ),
                (
                    "kw_variadic_convention",
                    option(declaration.kw_variadic_convention.map(convention)),
                ),
                (
                    "kw_variadic_index",
                    option(declaration.kw_variadic_index.map(|v| v.to_string())),
                ),
                (
                    "positional_only",
                    option(declaration.positional_only.map(|v| v.to_string())),
                ),
                (
                    "keyword_only",
                    option(declaration.keyword_only.map(|v| v.to_string())),
                ),
                (
                    "param_decls",
                    list(declaration.param_decls.iter().map(param_decl)),
                ),
                ("has_receiver", declaration.has_receiver.to_string()),
                (
                    "receiver_convention",
                    option(declaration.receiver_convention.map(convention)),
                ),
                (
                    "param_conventions",
                    list(
                        declaration
                            .param_conventions
                            .iter()
                            .map(|v| option(v.map(convention))),
                    ),
                ),
                ("return_type", ty_value(&declaration.ret_ty)),
                (
                    "returns_reference",
                    declaration.returns_reference.to_string(),
                ),
                ("raises", declaration.raises.to_string()),
                (
                    "error_type",
                    option(declaration.error_ty.as_ref().map(ty_value)),
                ),
                (
                    "ref_params",
                    list(declaration.ref_params.iter().map(bool::to_string)),
                ),
                (
                    "param_writes",
                    list(declaration.param_writes.iter().map(bool::to_string)),
                ),
                (
                    "availability",
                    list(declaration.availability.iter().map(constraint)),
                ),
                (
                    "enclosing",
                    option(declaration.enclosing.as_ref().map(|v| symbol(v))),
                ),
                (
                    "region_binders",
                    list(declaration.region_binders.iter().map(param_decl)),
                ),
            ]
            .into_iter()
            .chain(
                (declaration.inline != InlineLevel::Automatic)
                    .then(|| ("inline", declaration.inline.spelling().to_string())),
            )
            .chain(
                declaration
                    .is_synthesized()
                    .then(|| ("synthesized", "true".to_string())),
            )
            .collect::<Vec<_>>(),
        );
        write!(output, "    {value}").unwrap();
        writeln!(
            output,
            "{}",
            if index + 1 == declarations.len() {
                ""
            } else {
                ","
            }
        )
        .unwrap();
    }
    writeln!(output, "  ],").unwrap();
}

fn write_functions(
    output: &mut String,
    program: &MirProgram,
    files: &BTreeMap<Option<String>, usize>,
) {
    writeln!(output, "  functions: [").unwrap();
    for (index, (name, function)) in program.functions.iter().enumerate() {
        write_function(output, name, function, files, 2);
        writeln!(
            output,
            "{}",
            if index + 1 == program.functions.len() {
                ""
            } else {
                ","
            }
        )
        .unwrap();
    }
    writeln!(output, "  ]").unwrap();
}

fn write_function(
    output: &mut String,
    name: &str,
    function: &MirFunction,
    files: &BTreeMap<Option<String>, usize>,
    indent: usize,
) {
    let pad = "  ".repeat(indent);
    writeln!(output, "{pad}  fn {{").unwrap();
    writeln!(output, "{pad}    name: {},", symbol(name)).unwrap();
    writeln!(output, "{pad}    registers: {},", function.n_regs).unwrap();
    writeln!(output, "{pad}    vars: {},", function.n_vars).unwrap();
    writeln!(
        output,
        "{pad}    var_names: {},",
        list(function.var_names.iter().map(|v| symbol(v)))
    )
    .unwrap();
    writeln!(output, "{pad}    params: {},", function.n_params).unwrap();
    writeln!(
        output,
        "{pad}    param_types: {},",
        list(function.param_types.iter().map(ty_value))
    )
    .unwrap();
    writeln!(
        output,
        "{pad}    owned_params: {},",
        list(function.owned_params.iter().map(bool::to_string))
    )
    .unwrap();
    writeln!(
        output,
        "{pad}    deinit_params: {},",
        list(function.deinit_params.iter().map(bool::to_string))
    )
    .unwrap();
    writeln!(
        output,
        "{pad}    ref_params: {},",
        list(function.ref_params.iter().map(bool::to_string))
    )
    .unwrap();
    writeln!(
        output,
        "{pad}    returns_reference: {},",
        function.returns_reference
    )
    .unwrap();
    let var_types = function
        .var_tys
        .iter()
        .map(|(var, ty)| (*var, ty))
        .collect::<BTreeMap<_, _>>();
    writeln!(
        output,
        "{pad}    var_types: {},",
        list(var_types.into_iter().map(|(var, ty)| record(
            "var_type",
            &[("var", var_value(var)), ("type", ty_value(ty))]
        )))
    )
    .unwrap();
    writeln!(
        output,
        "{pad}    return_type: {},",
        option(function.ret_ty.as_ref().map(ty_value))
    )
    .unwrap();
    writeln!(output, "{pad}    raises: {},", function.raises).unwrap();
    writeln!(
        output,
        "{pad}    error_type: {},",
        option(function.error_ty.as_ref().map(ty_value))
    )
    .unwrap();
    // A raising function's error slot; a nonraising one has none.
    if let Some(slot) = function.error_slot {
        writeln!(output, "{pad}    error_slot: {},", var_value(slot)).unwrap();
    }
    let reg_types = function
        .reg_types
        .iter()
        .map(|(reg, ty)| (*reg, ty))
        .collect::<BTreeMap<_, _>>();
    writeln!(
        output,
        "{pad}    register_types: {},",
        list(reg_types.into_iter().map(|(reg, ty)| record(
            "reg_type",
            &[("reg", format!("%r{reg}")), ("type", ty_value(ty))]
        )))
    )
    .unwrap();
    let locations = (0..function.n_regs).map(|reg| {
        let location = function.spans.0.get(&reg).map(|(span, origin)| {
            let file = files
                .get(&span.source)
                .copied()
                .expect("source files were collected before writing");
            record(
                "loc",
                &[
                    ("file", format!("file{file}")),
                    ("start", span.span.0.to_string()),
                    ("end", span.span.1.to_string()),
                    ("origin", option(origin.map(var_value))),
                ],
            )
        });
        record(
            "reg_loc",
            &[("reg", format!("%r{reg}")), ("location", option(location))],
        )
    });
    writeln!(output, "{pad}    locations: {},", list(locations)).unwrap();
    if !function.removed_params.is_empty() {
        let removed = function.removed_params.iter().map(|id| {
            record(
                "param_id",
                &[("owner", quote(&id.owner)), ("slot", id.slot.to_string())],
            )
        });
        writeln!(output, "{pad}    removed_params: {},", list(removed)).unwrap();
    }
    if !function.callee_removed_params.is_empty() {
        let callees = function.callee_removed_params.iter().map(|(callee, ids)| {
            let params = ids.iter().map(|id| {
                record(
                    "param_id",
                    &[("owner", quote(&id.owner)), ("slot", id.slot.to_string())],
                )
            });
            record(
                "callee_params",
                &[("callee", quote(callee)), ("params", list(params))],
            )
        });
        writeln!(output, "{pad}    callee_removed_params: {},", list(callees)).unwrap();
    }
    if !function.removed_args.is_empty() {
        let removed = function.removed_args.iter().copied().map(var_value);
        writeln!(output, "{pad}    removed_args: {},", list(removed)).unwrap();
    }
    if !function.callee_removed_args.is_empty() {
        let callees = function.callee_removed_args.iter().map(|(callee, slots)| {
            let args = slots.iter().copied().map(var_value);
            record(
                "callee_args",
                &[("callee", quote(callee)), ("args", list(args))],
            )
        });
        writeln!(output, "{pad}    callee_removed_args: {},", list(callees)).unwrap();
    }
    writeln!(output, "{pad}    blocks: [").unwrap();
    write_blocks(output, &function.blocks, indent + 3);
    writeln!(output, "{pad}    ]").unwrap();
    write!(output, "{pad}  }}").unwrap();
}

fn write_blocks(output: &mut String, blocks: &[MirBlock], indent: usize) {
    let pad = "  ".repeat(indent);
    for (block_id, block) in blocks.iter().enumerate() {
        writeln!(output, "{pad}bb{block_id} {{").unwrap();
        writeln!(output, "{pad}  instructions: [").unwrap();
        for (index, instruction) in block.instrs.iter().enumerate() {
            write!(output, "{pad}    {}", instruction_value(instruction)).unwrap();
            writeln!(
                output,
                "{}",
                if index + 1 == block.instrs.len() {
                    ""
                } else {
                    ","
                }
            )
            .unwrap();
        }
        writeln!(output, "{pad}  ],").unwrap();
        writeln!(output, "{pad}  terminator: {}", term_value(&block.term)).unwrap();
        write!(output, "{pad}}}").unwrap();
        writeln!(
            output,
            "{}",
            if block_id + 1 == blocks.len() {
                ""
            } else {
                ","
            }
        )
        .unwrap();
    }
}

#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
fn instruction_value(instruction: &MirInstr) -> String {
    let tag = instruction_mnemonic(instruction);
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            loans,
            marker,
            dest_interior,
        } => record(
            tag,
            &[
                ("reference", var_value(*reference)),
                ("loans", list(loans.iter().map(loan))),
                ("marker", reg_value(*marker)),
                (
                    "dest_interior",
                    option(dest_interior.as_ref().map(interior_origin)),
                ),
            ],
        ),
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            marker,
        } => record(
            tag,
            &[
                ("base", interior_origin(base)),
                ("except", option(except.map(var_value))),
                (
                    "include_base_generation",
                    include_base_generation.to_string(),
                ),
                ("marker", reg_value(*marker)),
            ],
        ),
        MirInstr::MakeRef { dest, place } => record(
            tag,
            &[("dest", reg_value(*dest)), ("place", place_value(place))],
        ),
        MirInstr::ReadRef { dest, reference } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("reference", reg_value(*reference)),
            ],
        ),
        MirInstr::CopyValue {
            dest,
            value,
            constructor,
        } => record(
            tag,
            &with_symbol(
                vec![("dest", reg_value(*dest)), ("value", reg_value(*value))],
                "constructor",
                constructor.as_deref(),
            ),
        ),
        MirInstr::Rebind { dest, value } => record(
            tag,
            &[("dest", reg_value(*dest)), ("value", reg_value(*value))],
        ),
        MirInstr::WriteRef { reference, value } => record(
            tag,
            &[
                ("reference", reg_value(*reference)),
                ("value", reg_value(*value)),
            ],
        ),
        MirInstr::MakeClosure {
            dest,
            function,
            captures,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("function", symbol(function)),
                (
                    "captures",
                    list(captures.iter().map(|capture| {
                        record(
                            "closure_capture",
                            &with_symbol(
                                with_symbol(
                                    vec![
                                        ("place", place_value(&capture.place)),
                                        (
                                            "mode",
                                            match capture.mode {
                                                MirCaptureMode::Reference => "reference",
                                                MirCaptureMode::Move => "move",
                                            }
                                            .into(),
                                        ),
                                    ],
                                    "constructor",
                                    capture.constructor.as_deref(),
                                ),
                                "destructor",
                                capture.destructor.as_deref(),
                            ),
                        )
                    })),
                ),
            ],
        ),
        MirInstr::KeepAlive { var } => record(tag, &[("var", var_value(*var))]),
        MirInstr::Const { dest, k } => record(
            tag,
            &[("dest", reg_value(*dest)), ("value", const_value(k))],
        ),
        MirInstr::ParamListAddress { dest, values } => record(
            tag,
            &[("dest", reg_value(*dest)), ("values", const_value(values))],
        ),
        MirInstr::StoreToMem { dest, src } => {
            record(tag, &[("dest", reg_value(*dest)), ("src", reg_value(*src))])
        }
        MirInstr::IsRunInComptimeInterpreter { dest } => record(tag, &[("dest", reg_value(*dest))]),
        MirInstr::SizeOf { dest, ty }
        | MirInstr::TypeName { dest, ty }
        | MirInstr::ConstructType { dest, ty } => {
            record(tag, &[("dest", reg_value(*dest)), ("type", ty_value(ty))])
        }
        MirInstr::ConstructTypeParam {
            dest,
            param,
            kwargs,
            kwarg_places,
            element,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("owner", quote(&param.id.owner)),
                ("slot", param.id.slot.to_string()),
                ("param", symbol(&param.name)),
                ("kwargs", kwargs_value(kwargs)),
                ("kwarg_places", places_option(kwarg_places)),
                ("element", option(element.as_ref().map(param_arg))),
            ],
        ),
        MirInstr::MaterializeLiteral {
            dest,
            value,
            target,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("target", ty_value(target)),
            ],
        ),
        MirInstr::BuiltinCast { dest, value } => record(
            tag,
            &[("dest", reg_value(*dest)), ("value", reg_value(*value))],
        ),
        MirInstr::ScalarCast {
            dest,
            value,
            target,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("target", target.name().into()),
            ],
        ),
        MirInstr::Pop { dest, op, operands } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("op", op.mnemonic().into()),
                ("operands", regs(operands)),
            ],
        ),
        MirInstr::CallLlvmIntrinsic {
            dest,
            intrinsic,
            has_side_effect,
            args,
            spread,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("intrinsic", const_value(intrinsic)),
                ("has_side_effect", const_value(has_side_effect)),
                ("args", regs(args)),
                (
                    "spread",
                    option(spread.map(|position| position.to_string())),
                ),
            ],
        ),
        MirInstr::UseVar {
            dest,
            var,
            mode,
            constructor,
        } => record(
            tag,
            &with_symbol(
                vec![
                    ("dest", reg_value(*dest)),
                    ("var", var_value(*var)),
                    ("mode", super::use_mode_spelling(*mode).into()),
                ],
                "constructor",
                constructor.as_deref(),
            ),
        ),
        MirInstr::MovePlace { dest, place } => record(
            tag,
            &[("dest", reg_value(*dest)), ("place", place_value(place))],
        ),
        MirInstr::DefVar {
            var,
            src,
            binding_ty,
            in_place,
        } => record(
            tag,
            &[
                ("var", var_value(*var)),
                ("src", reg_value(*src)),
                ("binding_type", option(binding_ty.as_ref().map(ty_value))),
                ("in_place", in_place.to_string()),
            ],
        ),
        MirInstr::UnOp {
            op,
            dest,
            a,
            raises,
        } => record(
            tag,
            &[
                ("op", prefix(*op).into()),
                ("dest", reg_value(*dest)),
                ("a", reg_value(*a)),
                ("raises", option(raises.as_ref().map(raises_value))),
            ],
        ),
        MirInstr::BinOp {
            op,
            dest,
            a,
            b,
            resolved,
            raises,
            refined_bound,
        } => record(
            tag,
            &with_refined_bound(
                vec![
                    ("op", infix(*op).into()),
                    ("dest", reg_value(*dest)),
                    ("a", reg_value(*a)),
                    ("b", reg_value(*b)),
                    ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
                    ("raises", option(raises.as_ref().map(raises_value))),
                ],
                refined_bound
                    .as_deref()
                    .map(|bound| bound.traits.as_slice()),
            ),
        ),
        MirInstr::Call {
            dest,
            func,
            raises,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            receiver,
            instantiated_args,
            spread,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("func", symbol(&func.0)),
                ("raises", option(raises.as_ref().map(raises_value))),
                ("args", regs(args)),
                ("kwargs", kwargs_value(kwargs)),
                ("arg_places", places_option(arg_places)),
                ("kwarg_places", places_option(kwarg_places)),
                (
                    "capture_accesses",
                    list(capture_accesses.iter().map(capture_access)),
                ),
                ("param_arg_regs", list(param_arg_regs.iter().map(param_arg))),
                ("receiver", option(receiver.as_ref().map(ty_value))),
                (
                    "instantiated_args",
                    list(instantiated_args.iter().map(ty_arg)),
                ),
                (
                    "spread",
                    option(spread.map(|position| position.to_string())),
                ),
            ],
        ),
        MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            raises,
            args,
            kwargs,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
            spread,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("callee", reg_value(*callee)),
                ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
                ("raises", option(raises.as_ref().map(raises_value))),
                ("args", regs(args)),
                ("kwargs", kwargs_value(kwargs)),
                (
                    "callee_place",
                    option(callee_place.as_deref().map(place_value)),
                ),
                ("arg_places", places_option(arg_places)),
                ("kwarg_places", places_option(kwarg_places)),
                (
                    "capture_accesses",
                    list(capture_accesses.iter().map(capture_access)),
                ),
                ("param_arg_regs", list(param_arg_regs.iter().map(param_arg))),
                ("param_decls", list(param_decls.iter().map(param_decl))),
                (
                    "instantiated_contract",
                    option(instantiated_contract.as_ref().map(ty_value)),
                ),
                (
                    "instantiated_args",
                    list(instantiated_args.iter().map(ty_arg)),
                ),
                (
                    "spread",
                    option(spread.map(|position| position.to_string())),
                ),
            ],
        ),
        MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved,
            raises,
            reference_result,
            result_adapter,
            args,
            kwargs,
            recv_place,
            recv_writes,
            receiver_convention,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread,
            refined_bound,
        } => record(
            tag,
            &with_refined_bound(
                with_symbol(
                    vec![
                        ("dest", reg_value(*dest)),
                        ("recv", reg_value(*recv)),
                        ("method", symbol(method)),
                        ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
                        ("raises", option(raises.as_ref().map(raises_value))),
                        (
                            "reference_result",
                            option(reference_result.as_deref().map(ref_ty)),
                        ),
                        (
                            "result_adapter",
                            option(
                                result_adapter
                                    .as_deref()
                                    .map(|adapter| result_adapter_value(adapter.kind)),
                            ),
                        ),
                        ("args", regs(args)),
                        ("kwargs", kwargs_value(kwargs)),
                        ("recv_place", option(recv_place.as_deref().map(place_value))),
                        ("recv_writes", recv_writes.to_string()),
                        (
                            "receiver_convention",
                            option(receiver_convention.map(convention)),
                        ),
                        ("arg_places", places_option(arg_places)),
                        ("kwarg_places", places_option(kwarg_places)),
                        (
                            "capture_accesses",
                            list(capture_accesses.iter().map(capture_access)),
                        ),
                        ("param_arg_regs", list(param_arg_regs.iter().map(param_arg))),
                        ("param_decls", list(param_decls.iter().map(param_decl))),
                        (
                            "instantiated_args",
                            list(instantiated_args.iter().map(ty_arg)),
                        ),
                        (
                            "spread",
                            option(spread.map(|position| position.to_string())),
                        ),
                    ],
                    "adapter_constructor",
                    result_adapter
                        .as_deref()
                        .and_then(|adapter| adapter.constructor.as_deref()),
                ),
                refined_bound
                    .as_deref()
                    .map(|bound| bound.traits.as_slice()),
            ),
        ),
        MirInstr::PointerStorageTake {
            dest,
            pointer,
            index,
            element,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("pointer", reg_value(*pointer)),
                ("index", reg_value(*index)),
                ("element", ty_value(element)),
            ],
        ),
        MirInstr::PointerStorageDestroy {
            dest,
            pointer,
            index,
            element,
            destructor,
        } => record(
            tag,
            &with_symbol(
                vec![
                    ("dest", reg_value(*dest)),
                    ("pointer", reg_value(*pointer)),
                    ("index", reg_value(*index)),
                    ("element", ty_value(element)),
                ],
                "destructor",
                destructor.as_deref(),
            ),
        ),
        MirInstr::UninitStorage { dest, init } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("init", option(init.map(reg_value))),
            ],
        ),
        MirInstr::UninitStorageTake {
            dest,
            storage,
            element,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("storage", reg_value(*storage)),
                ("element", ty_value(element)),
            ],
        ),
        MirInstr::UninitStorageDestroy {
            dest,
            storage,
            element,
            destructor,
        } => record(
            tag,
            &with_symbol(
                vec![
                    ("dest", reg_value(*dest)),
                    ("storage", reg_value(*storage)),
                    ("element", ty_value(element)),
                ],
                "destructor",
                destructor.as_deref(),
            ),
        ),
        MirInstr::GetField { dest, base, field } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("base", reg_value(*base)),
                ("field", symbol(field)),
            ],
        ),
        MirInstr::Index {
            dest,
            base,
            index,
            base_place,
            index_place,
            call,
            intrinsic,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("base", reg_value(*base)),
                ("index", reg_value(*index)),
                ("base_place", option(base_place.as_deref().map(place_value))),
                (
                    "index_place",
                    option(index_place.as_deref().map(place_value)),
                ),
                ("call", option(call.as_deref().map(subscript_call))),
                (
                    "intrinsic",
                    option(intrinsic.map(|v| super::intrinsic_subscript_spelling(v).into())),
                ),
            ],
        ),
        MirInstr::MultiIndex {
            dest,
            object,
            args,
            object_place,
            arg_places,
            kwargs,
            kwarg_places,
            call,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("object", reg_value(*object)),
                ("args", list(args.iter().map(subscript_arg))),
                (
                    "object_place",
                    option(object_place.as_deref().map(place_value)),
                ),
                ("arg_places", places_option(arg_places)),
                (
                    "kwargs",
                    list(kwargs.iter().map(|(name, value)| {
                        record(
                            "keyword",
                            &[("name", symbol(name)), ("value", subscript_arg(value))],
                        )
                    })),
                ),
                ("kwarg_places", places_option(kwarg_places)),
                ("call", option(call.as_deref().map(subscript_call))),
            ],
        ),
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            value_keyword,
            call,
        } => record(
            tag,
            &[
                ("receiver", reg_value(*receiver)),
                (
                    "receiver_place",
                    option(receiver_place.as_deref().map(place_value)),
                ),
                ("args", list(args.iter().map(subscript_arg))),
                ("arg_places", places_option(arg_places)),
                ("value", reg_value(*value)),
                (
                    "value_place",
                    option(value_place.as_deref().map(place_value)),
                ),
                ("value_keyword", quote(value_keyword)),
                ("call", subscript_call(call)),
            ],
        ),
        MirInstr::Store { place, src } => record(
            tag,
            &[("place", place_value(place)), ("src", reg_value(*src))],
        ),
        MirInstr::StoreRef { place, reference } => record(
            tag,
            &[
                ("place", place_value(place)),
                ("reference", reg_value(*reference)),
            ],
        ),
        MirInstr::LoadPlace { dest, place } => record(
            tag,
            &[("dest", reg_value(*dest)), ("place", place_value(place))],
        ),
        MirInstr::MakeTuple {
            dest,
            elems,
            element_types,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("elems", regs(elems)),
                (
                    "element_types",
                    option(element_types.as_ref().map(|v| list(v.iter().map(ty_value)))),
                ),
            ],
        ),
        MirInstr::MakeStruct { dest, fields } => {
            record(tag, &[("dest", reg_value(*dest)), ("fields", regs(fields))])
        }
        MirInstr::MakeArray { dest, elems } => {
            record(tag, &[("dest", reg_value(*dest)), ("elems", regs(elems))])
        }
        MirInstr::MakeVariant {
            dest,
            alternatives,
            index,
            value,
            constructor,
        } => record(
            tag,
            &with_symbol(
                vec![
                    ("dest", reg_value(*dest)),
                    ("alternatives", list(alternatives.iter().map(ty_value))),
                    ("index", variant_index(index)),
                    ("value", reg_value(*value)),
                ],
                "constructor",
                constructor.as_deref(),
            ),
        ),
        MirInstr::VariantIs {
            dest,
            variant,
            index,
        }
        | MirInstr::VariantGet {
            dest,
            variant,
            index,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("variant", reg_value(*variant)),
                ("index", variant_index(index)),
            ],
        ),
        MirInstr::VariantTag { dest, variant } => record(
            tag,
            &[("dest", reg_value(*dest)), ("variant", reg_value(*variant))],
        ),
        MirInstr::VariantSet {
            dest,
            place,
            index,
            value,
            constructor,
            destructors,
        } => record(
            tag,
            &with_symbols(
                with_symbol(
                    vec![
                        ("dest", reg_value(*dest)),
                        ("place", place_value(place)),
                        ("index", variant_index(index)),
                        ("value", reg_value(*value)),
                    ],
                    "constructor",
                    constructor.as_deref(),
                ),
                "destructors",
                destructors,
            ),
        ),
        MirInstr::VariantTake {
            dest,
            variant,
            index,
            checked,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("variant", reg_value(*variant)),
                ("index", variant_index(index)),
                ("checked", checked.to_string()),
            ],
        ),
        MirInstr::VariantSetInitWith {
            dest,
            place,
            index,
            factory,
            destructors,
        } => record(
            tag,
            &with_symbols(
                vec![
                    ("dest", reg_value(*dest)),
                    ("place", place_value(place)),
                    ("index", variant_index(index)),
                    ("factory", reg_value(*factory)),
                ],
                "destructors",
                destructors,
            ),
        ),
        MirInstr::VariantDeinitWith {
            dest,
            variant,
            handler,
            index,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("variant", reg_value(*variant)),
                ("handler", reg_value(*handler)),
                ("index", variant_index(index)),
            ],
        ),
        MirInstr::VariantReplace {
            dest,
            place,
            input_index,
            output_index,
            value,
            checked,
            constructor,
        } => record(
            tag,
            &with_symbol(
                vec![
                    ("dest", reg_value(*dest)),
                    ("place", place_value(place)),
                    ("input_index", variant_index(input_index)),
                    ("output_index", variant_index(output_index)),
                    ("value", reg_value(*value)),
                    ("checked", checked.to_string()),
                ],
                "constructor",
                constructor.as_deref(),
            ),
        ),
        MirInstr::MakeSimd {
            dest,
            dtype,
            width,
            elems,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("dtype", simd_dtype_slot(dtype)),
                ("width", simd_width_slot(width)),
                ("elems", regs(elems)),
            ],
        ),
        MirInstr::SimdCast {
            dest,
            value,
            dtype,
            width,
        }
        | MirInstr::SimdBitcast {
            dest,
            value,
            dtype,
            width,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("dtype", simd_dtype_slot(dtype)),
                ("width", simd_width_slot(width)),
            ],
        ),
        MirInstr::SimdShuffle {
            dest,
            value,
            other,
            mask,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("other", option(other.map(reg_value))),
                ("mask", lane_mask(mask)),
            ],
        ),
        MirInstr::Select {
            dest,
            condition,
            on_true,
            on_false,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("condition", reg_value(*condition)),
                ("on_true", reg_value(*on_true)),
                ("on_false", reg_value(*on_false)),
            ],
        ),
        MirInstr::SimdSelect {
            dest,
            mask,
            on_true,
            on_false,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("mask", reg_value(*mask)),
                ("on_true", reg_value(*on_true)),
                ("on_false", reg_value(*on_false)),
            ],
        ),
        MirInstr::SimdExtract { dest, value, index } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("index", reg_value(*index)),
            ],
        ),
        MirInstr::SimdInsert {
            dest,
            value,
            element,
            index,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("element", reg_value(*element)),
                ("index", reg_value(*index)),
            ],
        ),
        MirInstr::SimdReduce { dest, value, op } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("op", op.mnemonic().into()),
            ],
        ),
        MirInstr::Try {
            label,
            error,
            results,
            body,
            handler_args,
            handler,
            else_args,
            orelse,
        } => record(
            tag,
            &[
                ("label", try_label(*label)),
                ("error", option(error.map(var_value))),
                ("results", regs(results)),
                ("body", blocks_value(body)),
                ("handler_arguments", regs(handler_args)),
                (
                    "handler",
                    option(handler.as_ref().map(|blocks| blocks_value(blocks))),
                ),
                ("else_arguments", regs(else_args)),
                ("orelse", option(orelse.as_ref().map(|v| blocks_value(v)))),
            ],
        ),
        MirInstr::If {
            results,
            cond,
            then,
            elifs,
            else_args,
            orelse,
        } => record(
            tag,
            &[
                ("results", list(results.iter().map(|v| reg_value(*v)))),
                ("condition", reg_value(*cond)),
                ("then", blocks_value(then)),
                (
                    "elifs",
                    list(elifs.iter().map(|elif| {
                        record(
                            "elif",
                            &[
                                ("condition_arguments", regs(&elif.cond_args)),
                                ("condition", blocks_value(&elif.cond)),
                                ("then_arguments", regs(&elif.then_args)),
                                ("then", blocks_value(&elif.then)),
                            ],
                        )
                    })),
                ),
                ("else_arguments", regs(else_args)),
                ("else", blocks_value(orelse)),
            ],
        ),
        MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body,
        } => record(
            tag,
            &[
                ("results", list(results.iter().map(|v| reg_value(*v)))),
                ("label", option(label.map(loop_label))),
                ("operands", list(operands.iter().map(|v| reg_value(*v)))),
                ("arguments", list(args.iter().map(|v| reg_value(*v)))),
                ("body", blocks_value(body)),
            ],
        ),
        MirInstr::ComptimeIf {
            results,
            conds,
            arms,
            orelse,
        } => record(
            tag,
            &[
                ("results", list(results.iter().map(|v| reg_value(*v)))),
                ("conditions", list(conds.iter().map(constraint))),
                ("arms", list(arms.iter().map(|arm| blocks_value(arm)))),
                ("else", blocks_value(orelse)),
            ],
        ),
        MirInstr::ComptimeFor {
            binder,
            source,
            operands,
            args,
            results,
            body,
            orelse,
        } => {
            let mut fields = vec![("results", regs(results)), ("binder", binder_ref(binder))];
            fields.extend(sequence_fields(source));
            fields.extend([
                ("operands", regs(operands)),
                ("arguments", regs(args)),
                ("body", blocks_value(body)),
                ("else", blocks_value(orelse)),
            ]);
            record(tag, &fields)
        }
        MirInstr::ComptimeAssert { cond } => record(tag, &[("condition", constraint(cond))]),
        MirInstr::Match {
            results,
            cases,
            else_args,
            orelse,
        } => record(
            tag,
            &[
                ("results", regs(results)),
                (
                    "cases",
                    list(cases.iter().map(|case| {
                        record(
                            "case",
                            &[
                                ("arguments", regs(&case.args)),
                                ("body", blocks_value(&case.body)),
                            ],
                        )
                    })),
                ),
                ("else_arguments", regs(else_args)),
                ("else", blocks_value(orelse)),
            ],
        ),
        MirInstr::Drop { reg, destructor } => record(
            tag,
            &with_symbol(
                vec![("reg", reg_value(*reg))],
                "destructor",
                destructor.as_deref(),
            ),
        ),
        MirInstr::DeclareVar { var } | MirInstr::ConsumeVar { var } => {
            record(tag, &[("var", var_value(*var))])
        }
        MirInstr::DropVar { var, destructor } => record(
            tag,
            &with_symbol(
                vec![("var", var_value(*var))],
                "destructor",
                destructor.as_deref(),
            ),
        ),
        MirInstr::DropPlace { place, destructor } => record(
            tag,
            &with_symbol(
                vec![("place", place_value(place))],
                "destructor",
                destructor.as_deref(),
            ),
        ),
        MirInstr::MarkInitialized { place } | MirInstr::MarkDestroyed { place } => {
            record(tag, &[("place", place_value(place))])
        }
        MirInstr::Unsupported(message) => record(tag, &[("message", quote(message))]),
        MirInstr::GetIter { source, dest } => record(
            tag,
            &[("source", var_value(*source)), ("dest", var_value(*dest))],
        ),
        MirInstr::HasNext { dest, iter } | MirInstr::Next { dest, iter } => record(
            tag,
            &[("dest", reg_value(*dest)), ("iter", var_value(*iter))],
        ),
    }
}

fn term_value(term: &MirTerm) -> String {
    let tag = terminator_mnemonic(term);
    match term {
        MirTerm::Return(value) => record(tag, &[("value", option(value.map(reg_value)))]),
        MirTerm::Unreachable | MirTerm::ErrorReturn => record(tag, &[]),
        MirTerm::TryRaise { label, values } => record(
            tag,
            &[("label", try_label(*label)), ("values", regs(values))],
        ),
        MirTerm::Yield(values)
        | MirTerm::TryYield(values)
        | MirTerm::ComptimeYield(values)
        | MirTerm::MatchNext(values)
        | MirTerm::MatchComplete(values)
        | MirTerm::ComptimeBreak(values)
        | MirTerm::ComptimeContinue(values) => record(
            tag,
            &[("values", list(values.iter().map(|v| reg_value(*v))))],
        ),
        MirTerm::ElifCond { cond, values } => record(
            tag,
            &[("condition", reg_value(*cond)), ("values", regs(values))],
        ),
        MirTerm::Break { label, values } | MirTerm::Continue { label, values } => record(
            tag,
            &[
                ("label", option(label.map(loop_label))),
                ("values", list(values.iter().map(|v| reg_value(*v)))),
            ],
        ),
    }
}

/// A `comptime for` source's fields.
fn sequence_fields(source: &ComptimeSequence) -> Vec<(&'static str, String)> {
    vec![
        ("initial", param_expr(&source.initial)),
        ("has_next", param_expr(&source.has_next)),
        ("next_iter", param_expr(&source.next_iter)),
        ("next_value", param_expr(&source.next_value)),
    ]
}

/// A lifecycle site's fields: its operands, then the `destructor` a drop or
/// the `constructor` (a result adapter's `adapter_constructor`) a copy or
/// move names, omitted when it names none.
fn with_symbol<'a>(
    mut fields: Vec<(&'a str, String)>,
    key: &'a str,
    member: Option<&str>,
) -> Vec<(&'a str, String)> {
    fields.extend(member.map(|member| (key, symbol(member))));
    fields
}

/// A call's fields, then the `refined_bound` it was selected through,
/// omitted while it records none.
fn with_refined_bound<'a>(
    mut fields: Vec<(&'a str, String)>,
    bound: Option<&[String]>,
) -> Vec<(&'a str, String)> {
    fields
        .extend(bound.map(|bound| ("refined_bound", list(bound.iter().map(|name| symbol(name))))));
    fields
}

/// A variant store's fields, then the per-alternative `destructors` it
/// names, each present or absent, omitted while it names none.
fn with_symbols<'a>(
    mut fields: Vec<(&'a str, String)>,
    key: &'a str,
    members: &[Option<String>],
) -> Vec<(&'a str, String)> {
    if !members.is_empty() {
        fields.push((
            key,
            list(
                members
                    .iter()
                    .map(|member| option(member.as_deref().map(symbol))),
            ),
        ));
    }
    fields
}

fn loop_label(label: LoopLabel) -> String {
    format!("loop{}", label.0)
}

fn try_label(label: TryLabel) -> String {
    format!("try{}", label.0)
}

fn blocks_value(blocks: &[MirBlock]) -> String {
    list(blocks.iter().enumerate().map(|(id, block)| {
        record(
            &format!("bb{id}"),
            &[
                (
                    "instructions",
                    list(block.instrs.iter().map(instruction_value)),
                ),
                ("terminator", term_value(&block.term)),
            ],
        )
    }))
}

fn place_value(place: &MirPlace) -> String {
    let projections = place
        .proj
        .iter()
        .zip(&place.projection_tys)
        .map(|(projection, ty)| {
            record(
                "projection",
                &[("op", projection_value(projection)), ("type", ty_value(ty))],
            )
        });
    record(
        "place",
        &[
            ("root", var_value(place.root)),
            ("root_type", option(place.root_ty.as_ref().map(ty_value))),
            ("projections", list(projections)),
            ("type", option(place.ty.as_ref().map(ty_value))),
            ("through", option(place.through.map(var_value))),
        ],
    )
}

fn projection_value(projection: &Proj) -> String {
    match projection {
        Proj::Field(v) => positional("field", &symbol(v)),
        Proj::Index(v) => positional("index", &reg_value(*v)),
        Proj::ConstIndex(v) => positional("const_index", &v.to_string()),
        Proj::Variant(v) => positional("variant", &variant_index(v)),
        Proj::UninitPayload => "uninit_payload".into(),
        Proj::Rebind => "rebind".into(),
    }
}
fn loan(value: &MirLoan) -> String {
    record(
        "loan",
        &[
            ("place", place_value(&value.place)),
            ("mutable", value.mutable.to_string()),
            (
                "interior",
                option(value.interior.as_ref().map(interior_origin)),
            ),
            ("shared", value.shared.to_string()),
            ("setup", value.setup.to_string()),
        ],
    )
}
fn interior_origin(value: &MirInteriorOrigin) -> String {
    record(
        "interior_origin",
        &[
            ("root", var_value(value.root)),
            ("path", list(value.path.iter().map(origin_seg))),
        ],
    )
}
fn capture_access(value: &MirCaptureAccess) -> String {
    record(
        "capture_access",
        &[
            ("root", var_value(value.root)),
            ("path", list(value.path.iter().map(origin_seg))),
            ("access", capture_access_kind(value.access).into()),
        ],
    )
}
fn parameterized_associated_type(
    value: &mojito_checked::declaration::ParameterizedAssociatedType,
) -> String {
    use mojito_checked::declaration::AssociatedParameter;
    record(
        "parameterized_associated_type",
        &[
            ("name", symbol(&value.name)),
            (
                "params",
                list(value.params.iter().map(|param| match param {
                    AssociatedParameter::Type(binder) => {
                        positional("type_param", &binder_ref(binder))
                    }
                    AssociatedParameter::Value(name) => positional("value_param", &symbol(name)),
                    AssociatedParameter::Origin(id) => {
                        positional("origin_param", &id.0.to_string())
                    }
                })),
            ),
            ("type", ty_value(&value.template)),
        ],
    )
}
fn parameterized_associated_value(
    value: &mojito_checked::declaration::ParameterizedAssociatedValue,
) -> String {
    record(
        "parameterized_associated_value",
        &[
            ("name", symbol(&value.name)),
            ("params", list(value.params.iter().map(param_decl))),
            ("value", param_expr(&value.body)),
        ],
    )
}
fn binder_ref(value: &mojito_types::param_expr::ParamRef) -> String {
    record(
        "binder",
        &[
            ("owner", quote(&value.id.owner)),
            ("slot", value.id.slot.to_string()),
            ("name", symbol(&value.name)),
        ],
    )
}
fn param_arg(value: &MirParamArg) -> String {
    record(
        "param_arg",
        &[
            ("name", option(value.name.as_ref().map(|v| symbol(v)))),
            ("value", option(value.value.map(reg_value))),
            ("expr", option(value.expr.as_ref().map(param_expr))),
        ],
    )
}
fn subscript_arg(value: &MirSubscriptArg) -> String {
    match value {
        MirSubscriptArg::Index(v) => positional("index", &reg_value(*v)),
    }
}

fn subscript_call(value: &MirSubscriptCall) -> String {
    record(
        "subscript_call",
        &with_refined_bound(
            vec![
                ("target", symbol(&value.target)),
                ("raises", option(value.raises.as_ref().map(raises_value))),
                ("result_type", ty_value(&value.result_ty)),
                (
                    "receiver_requires_place",
                    value.receiver_requires_place.to_string(),
                ),
                (
                    "receiver_convention",
                    option(value.receiver_convention.map(convention)),
                ),
                ("arguments", list(value.arguments.iter().map(call_argument))),
                (
                    "capture_accesses",
                    list(value.capture_accesses.iter().map(capture_access)),
                ),
                (
                    "reference_result",
                    option(value.reference_result.as_ref().map(ref_ty)),
                ),
                (
                    "param_arg_regs",
                    list(value.param_arg_regs.iter().map(param_arg)),
                ),
                (
                    "param_decls",
                    list(value.param_decls.iter().map(param_decl)),
                ),
            ],
            value.refined_bound.as_deref(),
        ),
    )
}
fn call_argument(value: &CheckedCallArgument) -> String {
    record(
        "call_argument",
        &[
            (
                "source",
                match value.source {
                    CheckedCallArgumentSource::Positional(v) => {
                        positional("positional", &v.to_string())
                    }
                    CheckedCallArgumentSource::Keyword(v) => positional("keyword", &v.to_string()),
                    CheckedCallArgumentSource::Default => "default".into(),
                },
            ),
            ("parameter_type", ty_value(&value.parameter_ty)),
            ("requires_place", value.requires_place.to_string()),
            ("convention", option(value.convention.map(convention))),
        ],
    )
}
fn raises_value(value: &Raises) -> String {
    record(
        "raises",
        &[
            ("type", ty_value(&value.ty)),
            ("flag", reg_value(value.flag)),
            ("error", reg_value(value.error)),
        ],
    )
}
fn result_adapter_value(value: CheckedResultAdapter) -> String {
    match value {
        CheckedResultAdapter::CopyIteratorReference => "copy_iterator_reference".into(),
    }
}

fn ty_value(ty: &Ty) -> String {
    match ty.kind() {
        TyKind::INT => "Int".into(),
        TyKind::UINT => "UInt".into(),
        TyKind::Bool => "Bool".into(),
        TyKind::StringLiteral => "StringLiteral".into(),
        TyKind::FLOAT64 => "Float64".into(),
        TyKind::None => "None".into(),
        TyKind::Never => "Never".into(),
        TyKind::IntLiteral => "IntLiteral".into(),
        TyKind::FloatLiteral => "FloatLiteral".into(),
        TyKind::Infer => "Infer".into(),
        TyKind::Dtype => "DType".into(),
        TyKind::TypeType => "TypeType".into(),
        TyKind::KgenString => "KgenString".into(),
        TyKind::KgenDeferred => "KgenDeferred".into(),
        TyKind::Index => "index".into(),
        TyKind::I1 => "i1".into(),
        TyKind::SelfType => "Self".into(),
        TyKind::Error => "Error".into(),
        TyKind::Func(func) => {
            let FuncTy {
                environment,
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
                transfers,
            } = &**func;
            callable_type(
                "func",
                environment,
                &[],
                params,
                names,
                ret,
                required,
                variadic.as_ref(),
                kw_variadic.as_ref(),
                *positional_only,
                *keyword_only,
                *raises,
                error.as_ref(),
                conventions,
                ref_params,
                ref_return.as_deref(),
                transfers,
            )
        }
        TyKind::GenericFunc(generic) => {
            let GenericFuncTy {
                decls,
                func:
                    FuncTy {
                        environment,
                        params,
                        names,
                        ret,
                        required,
                        variadic,
                        kw_variadic,
                        positional_only,
                        keyword_only,
                        raises,
                        error,
                        conventions,
                        ref_params,
                        ref_return,
                        transfers,
                    },
            } = &**generic;
            callable_type(
                "generic_func",
                environment,
                decls,
                params,
                names,
                ret,
                required,
                variadic.as_ref(),
                kw_variadic.as_ref(),
                *positional_only,
                *keyword_only,
                *raises,
                error.as_ref(),
                conventions,
                ref_params,
                ref_return.as_deref(),
                transfers,
            )
        }
        TyKind::Overload(values) => positional("overload", &list(values.iter().map(ty_value))),
        TyKind::Generator(generator) => record(
            "generator",
            &[
                ("params", list(generator.params.iter().map(param_decl))),
                ("result", ty_value(&generator.result)),
            ],
        ),
        TyKind::Param {
            binder,
            bounds,
            callable_bound,
        } => record(
            "param",
            &[
                ("owner", quote(&binder.id.owner)),
                ("slot", binder.id.slot.to_string()),
                ("name", symbol(&binder.name)),
                ("bounds", list(bounds.iter().map(|v| symbol(v)))),
                (
                    "callable_bound",
                    option(callable_bound.as_ref().map(ty_value)),
                ),
            ],
        ),
        TyKind::Assoc { base, name, args } => record(
            "assoc",
            &[
                ("base", ty_value(base)),
                ("member", symbol(name)),
                ("arguments", list(args.iter().map(ty_arg))),
            ],
        ),
        TyKind::Dependent(DependentType::Parameter(expr)) => {
            positional("dependent_parameter", &param_expr(expr))
        }
        TyKind::Struct(name, args) => record(
            "struct_type",
            &[
                ("name", symbol(name)),
                ("arguments", list(args.iter().map(ty_arg))),
            ],
        ),
        // A symbolic slot is spelled as its parameter expression; the
        // verifier keeps one out of every artifact, so the form only serves
        // lossless round trips.
        TyKind::Simd { dtype, width } => record(
            "simd",
            &[
                ("dtype", simd_dtype_slot(dtype)),
                ("width", simd_width_slot(width)),
            ],
        ),
        TyKind::KgenSimd { dtype, width } => record(
            "kgen_simd",
            &[
                ("dtype", simd_dtype_slot(dtype)),
                ("width", simd_width_slot(width)),
            ],
        ),
        TyKind::ComptimeList(value) => positional("comptime_list", &ty_value(value)),
        TyKind::ParamList(value) => positional("param_list", &ty_value(value)),
        TyKind::AnyTrait(traits) => {
            positional("any_trait", &list(traits.iter().map(|v| symbol(v))))
        }
        TyKind::Trait(traits) => positional("trait_type", &list(traits.iter().map(|v| symbol(v)))),
        TyKind::Tuple(values) => positional("tuple", &list(values.iter().map(ty_value))),
        TyKind::PopArray { size, element } => record(
            "pop_array",
            &[("size", size.to_string()), ("element", ty_value(element))],
        ),
        TyKind::RuntimePack(values) => {
            positional("runtime_pack", &list(values.iter().map(ty_value)))
        }
        TyKind::VariadicPack(value) => positional("variadic_pack", &ty_value(value)),
        TyKind::Variant(values) => positional("variant", &list(values.iter().map(ty_value))),
        TyKind::Pointer { element, origin } => record(
            "pointer",
            &[
                ("element", ty_value(element)),
                ("origin", pointer_origin(origin)),
            ],
        ),
        TyKind::Ref(value) => ref_ty(value),
    }
}

#[allow(clippy::too_many_arguments)]
fn callable_type(
    tag: &str,
    environment: &CallableEnvironment,
    decls: &[ParamDecl],
    params: &[Ty],
    names: &[String],
    ret: &Ty,
    required: &[bool],
    variadic: Option<&Ty>,
    kw_variadic: Option<&Ty>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    raises: bool,
    error: Option<&Ty>,
    conventions: &[Option<ArgConvention>],
    ref_params: &[Option<RefSig>],
    ref_return: Option<&RefSig>,
    transfers: &mojito_checked::checked::TransferSet,
) -> String {
    record(
        tag,
        &[
            ("environment", environment_value(environment)),
            ("param_decls", list(decls.iter().map(param_decl))),
            ("params", list(params.iter().map(ty_value))),
            ("names", list(names.iter().map(|v| symbol(v)))),
            ("return_type", ty_value(ret)),
            ("required", list(required.iter().map(bool::to_string))),
            ("variadic", option(variadic.map(ty_value))),
            ("kw_variadic", option(kw_variadic.map(ty_value))),
            (
                "positional_only",
                option(positional_only.map(|v| v.to_string())),
            ),
            ("keyword_only", option(keyword_only.map(|v| v.to_string()))),
            ("raises", raises.to_string()),
            ("error_type", option(error.map(ty_value))),
            (
                "conventions",
                list(conventions.iter().map(|v| option(v.map(convention)))),
            ),
            (
                "ref_params",
                list(ref_params.iter().map(|v| option(v.as_ref().map(ref_sig)))),
            ),
            ("ref_return", option(ref_return.map(ref_sig))),
            ("transfers", transfer_set(transfers)),
        ],
    )
}

fn transfer_set(value: &mojito_checked::checked::TransferSet) -> String {
    list(value.iter().map(|effect| {
        record(
            "transfer",
            &[
                ("dest", sig_origin(&effect.dest)),
                ("src", sig_origin(&effect.src)),
                ("src_is_place", effect.src_is_place.to_string()),
                ("mutable", effect.mutable.to_string()),
            ],
        )
    }))
}

fn ty_arg(value: &TyArg) -> String {
    match value {
        TyArg::Ty(v) => positional("type_arg", &ty_value(v)),
        TyArg::Val(v) => positional("value_arg", &ct_value(v)),
        TyArg::Origin(v) => positional("origin_arg", &origin(v)),
    }
}
fn param_decl(value: &ParamDecl) -> String {
    match value {
        ParamDecl::Type {
            id,
            name,
            bounds,
            trait_param,
            callable_bound,
            default,
            infer_only,
            variadic,
            constraints,
        } => record(
            "type_param",
            &[
                ("owner", quote(&id.owner)),
                ("slot", id.slot.to_string()),
                ("name", symbol(name)),
                ("bounds", list(bounds.iter().map(|v| symbol(v)))),
                ("trait_param", option(trait_param.as_ref().map(binder_ref))),
                (
                    "callable_bound",
                    option(callable_bound.as_ref().map(ty_value)),
                ),
                ("default", option(default.as_ref().map(ty_value))),
                ("infer_only", infer_only.to_string()),
                ("variadic", variadic.to_string()),
                ("constraints", list(constraints.iter().map(constraint))),
            ],
        ),
        ParamDecl::Value {
            id,
            name,
            ty,
            default,
            callable_default,
            infer_only,
            variadic,
            constraints,
        } => record(
            "value_param",
            &[
                ("owner", quote(&id.owner)),
                ("slot", id.slot.to_string()),
                ("name", symbol(name)),
                ("type", ty_value(ty)),
                ("default", option(default.as_ref().map(param_expr))),
                (
                    "callable_default",
                    option(callable_default.as_ref().map(callable_default_value)),
                ),
                ("infer_only", infer_only.to_string()),
                ("variadic", variadic.to_string()),
                ("constraints", list(constraints.iter().map(constraint))),
            ],
        ),
    }
}

fn constraint(value: &GenericConstraint) -> String {
    match value {
        GenericConstraint::WithMessage(condition, message) => record(
            "with_message",
            &[
                ("condition", constraint(condition)),
                ("message", quote(message)),
            ],
        ),
        GenericConstraint::Conforms {
            operand,
            trait_name,
        } => record(
            "conforms",
            &[
                ("operand", constraint_operand(operand)),
                ("trait", symbol(trait_name)),
            ],
        ),
        GenericConstraint::ConformsPack {
            operand,
            trait_name,
        } => record(
            "conforms_pack",
            &[
                ("operand", constraint_operand(operand)),
                ("trait", symbol(trait_name)),
            ],
        ),
        GenericConstraint::PackPredicate {
            operand,
            predicate,
            all,
        } => record(
            "pack_predicate",
            &[
                ("operand", constraint_operand(operand)),
                ("predicate", pack_predicate(predicate)),
                ("all", all.to_string()),
            ],
        ),
        GenericConstraint::PackContains { operand, element } => record(
            "pack_contains",
            &[
                ("operand", constraint_operand(operand)),
                ("element", constraint_operand(element)),
            ],
        ),
        GenericConstraint::Trivial(kind, operand) => record(
            "trivial",
            &[
                ("lifecycle", lifecycle(*kind).into()),
                ("operand", constraint_operand(operand)),
            ],
        ),
        GenericConstraint::Eq(left, right) => comparison("eq", left, right),
        GenericConstraint::Ne(left, right) => comparison("ne", left, right),
        GenericConstraint::Lt(left, right) => comparison("lt", left, right),
        GenericConstraint::Le(left, right) => comparison("le", left, right),
        GenericConstraint::Gt(left, right) => comparison("gt", left, right),
        GenericConstraint::Ge(left, right) => comparison("ge", left, right),
        GenericConstraint::And(left, right) => logical("and", left, right),
        GenericConstraint::Or(left, right) => logical("or", left, right),
        GenericConstraint::Not(value) => positional("not", &constraint(value)),
        GenericConstraint::Bool(value) => positional("constraint_bool", &value.to_string()),
    }
}

fn comparison(tag: &str, left: &ConstraintOperand, right: &ConstraintOperand) -> String {
    record(
        tag,
        &[
            ("left", constraint_operand(left)),
            ("right", constraint_operand(right)),
        ],
    )
}

fn logical(tag: &str, left: &GenericConstraint, right: &GenericConstraint) -> String {
    record(
        tag,
        &[("left", constraint(left)), ("right", constraint(right))],
    )
}

fn constraint_operand(value: &ConstraintOperand) -> String {
    match value {
        ConstraintOperand::Param(value) => positional("operand_param", &binder_ref(value)),
        ConstraintOperand::Value(value) => positional("operand_value", &ct_value(value)),
        ConstraintOperand::Type(value) => positional("operand_type", &ty_value(value)),
        ConstraintOperand::PackLength(value) => {
            positional("operand_pack_length", &constraint_operand(value))
        }
        ConstraintOperand::Expr(value) => positional("operand_expr", &param_expr(value)),
    }
}

fn pack_predicate(value: &PackPredicateRef) -> String {
    match value {
        PackPredicateRef::Trivial(value) => positional("predicate_trivial", lifecycle(*value)),
        PackPredicateRef::Alias(value) => positional("predicate_alias", &symbol(value)),
    }
}

const fn lifecycle(value: TrivialLifecycle) -> &'static str {
    match value {
        TrivialLifecycle::Movable => "movable",
        TrivialLifecycle::Copyable => "copyable",
        TrivialLifecycle::Deinitable => "deinitable",
    }
}

fn callable_default_value(value: &CallableDefault) -> String {
    match value {
        CallableDefault::Symbol(v) => positional("default_symbol", &symbol(v)),
        CallableDefault::Parameter(v) => positional("default_parameter", &binder_ref(v)),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => record(
            "default_if",
            &[
                ("condition", param_expr(condition)),
                ("then_value", callable_default_value(then_value)),
                ("else_value", callable_default_value(else_value)),
            ],
        ),
    }
}

fn const_value(value: &Const) -> String {
    match value {
        Const::Int(v) => positional("int", &v.to_string()),
        Const::Float(v) => positional("float", &format!("{:016x}", v.to_bits())),
        Const::IntLiteral(v) => positional("int_literal", &v.to_string()),
        Const::FloatLiteral(v) => positional("float_literal", &v.to_string()),
        Const::Bool(v) => positional("bool", &v.to_string()),
        Const::Str(v) => positional("string", &quote(v)),
        Const::Function(v) => positional("function", &symbol(v)),
        Const::FunctionInstance {
            function,
            arguments,
        } => record(
            "function_instance",
            &[
                ("function", symbol(function)),
                ("arguments", list(arguments.iter().map(ty_arg))),
            ],
        ),
        Const::Dtype(v) => positional("dtype", v.name()),
        Const::None => "none".into(),
        Const::Param(v) => positional("param", &param_expr(v)),
        Const::Value(v) => positional("value", &ct_value(v)),
        Const::UninitMem => "uninitmem".into(),
    }
}
fn checked_const(value: &CheckedConst) -> String {
    match value {
        CheckedConst::Int(v) => positional("checked_int", &v.to_string()),
        CheckedConst::Float(v) => positional("checked_float", &v.to_string()),
        CheckedConst::Bool(v) => positional("checked_bool", &v.to_string()),
        CheckedConst::String(v) => positional("checked_string", &quote(v)),
        CheckedConst::None => "checked_none".into(),
        CheckedConst::Dtype(v) => positional("checked_dtype", v.name()),
        CheckedConst::Construct { target, arg } => record(
            "checked_construct",
            &[("target", quote(target)), ("arg", checked_const(arg))],
        ),
        CheckedConst::Evaluate {
            function,
            arguments,
        } if arguments.is_empty() => positional("checked_evaluate", &quote(function)),
        CheckedConst::Evaluate {
            function,
            arguments,
        } => record(
            "checked_evaluate",
            &[
                ("function", quote(function)),
                ("arguments", list(arguments.iter().map(ty_arg))),
            ],
        ),
    }
}
/// A parameter expression in schema 1.1's typed structural form. Operands
/// print in canonical order, references by owner symbol and slot (or by
/// signature depth and index); no address or process-local key is written.
/// A vector's lane dtype: the dtype's name, or `ct_expr(...)` while the slot
/// is a parameter expression (the `simd` type record and the SIMD
/// instructions spell it alike).
fn simd_dtype_slot(dtype: &mojito_types::types::SimdDtype) -> String {
    match dtype {
        mojito_types::types::SimdDtype::Known(dtype) => dtype.name().into(),
        mojito_types::types::SimdDtype::Expr(expr) => positional("ct_expr", &param_expr(expr)),
    }
}

/// A vector's width: the lane count, or `ct_expr(...)` while symbolic.
fn simd_width_slot(width: &mojito_types::types::SimdWidth) -> String {
    match width {
        mojito_types::types::SimdWidth::Known(width) => width.to_string(),
        mojito_types::types::SimdWidth::Expr(expr) => positional("ct_expr", &param_expr(expr)),
    }
}

/// A known alternative is its position, a template's `ct_expr(...)`.
fn variant_index(index: &mojito_types::types::VariantIndex) -> String {
    match index {
        mojito_types::types::VariantIndex::Known(index) => index.to_string(),
        mojito_types::types::VariantIndex::Expr(expr) => positional("ct_expr", &param_expr(expr)),
    }
}

/// A known mask is its index list; a template's keeps its method's form.
fn lane_mask(mask: &mojito_types::types::LaneMask) -> String {
    use mojito_types::types::LaneMask;
    match mask {
        LaneMask::Known(lanes) => list(lanes.iter().map(usize::to_string)),
        LaneMask::Shuffle(lanes) => record(
            "lane_shuffle",
            &[("lanes", list(lanes.iter().map(param_expr)))],
        ),
        LaneMask::Slice { start, count } => record(
            "lane_slice",
            &[
                ("offset", param_expr(start)),
                ("width", simd_width_slot(count)),
            ],
        ),
        LaneMask::Join => record("lane_join", &[]),
    }
}

fn param_expr(value: &ParamExpr) -> String {
    let meta = ("type", meta_ty(value.meta()));
    match value.kind() {
        ParamKind::Constant(constant) => positional("param_constant", &ct_value(constant)),
        ParamKind::DeclRef(reference) => record(
            "param_decl_ref",
            &[
                ("owner", quote(&reference.id.owner)),
                ("slot", reference.id.slot.to_string()),
                ("name", symbol(&reference.name)),
                meta,
            ],
        ),
        ParamKind::IndexRef { depth, index } => record(
            "param_index_ref",
            &[
                ("depth", depth.to_string()),
                ("index", index.to_string()),
                meta,
            ],
        ),
        ParamKind::Op { op, operands } => record(
            "param_expr",
            &[
                ("op", op.name().into()),
                meta,
                ("operands", list(operands.iter().map(param_expr))),
            ],
        ),
        ParamKind::Identical(left, right) => record(
            "param_identical",
            &[("left", param_expr(left)), ("right", param_expr(right))],
        ),
        ParamKind::Conforms {
            subject,
            trait_name,
        } => record(
            "param_conforms",
            &[
                ("subject", param_expr(subject)),
                ("trait", symbol(trait_name)),
            ],
        ),
        ParamKind::Trivial { lifecycle, subject } => record(
            "param_trivial",
            &[
                ("lifecycle", self::lifecycle(*lifecycle).into()),
                ("subject", param_expr(subject)),
            ],
        ),
        ParamKind::TypeShape(ty) => positional("param_type_shape", &ty_value(ty)),
        ParamKind::Select { elements, index } => record(
            "param_select",
            &[
                ("elements", list(elements.iter().map(ty_value))),
                ("index", param_expr(index)),
            ],
        ),
        ParamKind::ListGet { list, index } => record(
            "param_list_get",
            &[("list", param_expr(list)), ("index", param_expr(index))],
        ),
        ParamKind::Field { base, name } => record(
            "param_field",
            &[("base", param_expr(base)), ("name", symbol(name)), meta],
        ),
        ParamKind::ListTabulate { count, element } => record(
            "param_list_tabulate",
            &[
                ("count", param_expr(count)),
                ("element", param_expr(element)),
            ],
        ),
        ParamKind::ListConcat { lists } => record(
            "param_list_concat",
            &[("lists", list(lists.iter().map(param_expr)))],
        ),
        ParamKind::Reflect { subject, query } => record(
            "param_reflect",
            &[
                ("subject", param_expr(subject)),
                ("query", symbol(&query.to_string())),
            ],
        ),
        ParamKind::TypeMember {
            subject,
            name,
            args,
        } => {
            let mut fields = vec![("subject", param_expr(subject)), ("name", symbol(name))];
            if !args.is_empty() {
                fields.push(("args", list(args.iter().map(param_expr))));
            }
            fields.push(meta);
            record("param_type_member", &fields)
        }
        ParamKind::PackQuery { pack, query } => record(
            "param_pack_query",
            &[("pack", binder_ref(pack)), ("query", pack_query(query))],
        ),
        ParamKind::Apply { function, args } => record(
            "param_apply",
            &[
                ("function", quote(function)),
                meta,
                ("args", list(args.iter().map(param_expr))),
            ],
        ),
        ParamKind::BindParams { generator, args } => record(
            "param_bind_params",
            &[
                ("generator", param_expr(generator)),
                (
                    "args",
                    list(args.iter().map(|arg| option(arg.as_ref().map(param_expr)))),
                ),
                meta,
            ],
        ),
        ParamKind::StructValue { fields, spelling } => record(
            "param_struct_value",
            &[
                (
                    "fields",
                    list(fields.iter().map(|(name, value)| {
                        record(
                            "field",
                            &[("name", symbol(name)), ("value", param_expr(value))],
                        )
                    })),
                ),
                ("spelling", param_expr(&spelling.0)),
                meta,
            ],
        ),
        // A hole never crosses into MIR: the parser rejects the form, and the
        // token keeps the text honest if one is reached.
        ParamKind::Hole { kind, token } => record(
            "param_hole",
            &[
                ("kind", format!("{kind:?}").to_lowercase()),
                ("token", token.to_string()),
                meta,
            ],
        ),
    }
}

fn pack_query(query: &PackQuery) -> String {
    match query {
        PackQuery::Length => "pack_length".into(),
        PackQuery::Conforms(trait_name) => positional("pack_conforms", &symbol(trait_name)),
        PackQuery::Predicate { predicate, all } => record(
            "pack_predicate",
            &[
                ("predicate", pack_predicate(predicate)),
                ("all", all.to_string()),
            ],
        ),
        PackQuery::Contains(element) => positional("pack_contains", &param_expr(element)),
        PackQuery::IndexOf(element) => positional("pack_index_of", &param_expr(element)),
    }
}

fn meta_ty(meta: &MetaTy) -> String {
    let metas = |metas: &[MetaTy]| list(metas.iter().map(meta_ty));
    match meta {
        MetaTy::Value(ty) => positional("meta_value", &ty_value(ty)),
        MetaTy::Type => "meta_type".into(),
        MetaTy::Tuple(elements) => positional("meta_tuple", &metas(elements)),
        MetaTy::List(elements) => positional("meta_list", &metas(elements)),
        MetaTy::Set(elements) => positional("meta_set", &metas(elements)),
        MetaTy::Dict(entries) => positional(
            "meta_dict",
            &list(entries.iter().map(|(key, value)| {
                record(
                    "meta_entry",
                    &[("key", meta_ty(key)), ("value", meta_ty(value))],
                )
            })),
        ),
        MetaTy::ParamList(element) => positional("meta_param_list", &meta_ty(element)),
    }
}

fn ct_value(value: &CtValue) -> String {
    match value {
        CtValue::Pointer { memory, offset } => record(
            "ct_pointer",
            &[
                ("offset", offset.to_string()),
                (
                    "slots",
                    memory.as_ref().map_or_else(
                        || "dangling".to_string(),
                        |slots| {
                            list(slots.iter().map(|slot| {
                                slot.as_ref().map_or_else(|| "absent".to_string(), ct_value)
                            }))
                        },
                    ),
                ),
            ],
        ),
        CtValue::StoreToMem(referent) => positional("ct_store_to_mem", &ct_value(referent)),
        CtValue::Int(v) => positional("ct_int", &v.to_string()),
        CtValue::UInt(v) => positional("ct_uint", &v.to_string()),
        CtValue::Index(v) => positional("ct_index", &v.to_string()),
        CtValue::Float(v) => positional("ct_float_bits", &format!("{v:016x}")),
        CtValue::IntLiteral(v) => positional("ct_int_literal", &v.to_string()),
        CtValue::FloatLiteral(v) => positional("ct_float_literal", &v.to_string()),
        CtValue::Bool(v) => positional("ct_bool", &v.to_string()),
        CtValue::Str(v) => positional("ct_string", &quote(v)),
        CtValue::Tuple(v) => positional("ct_tuple", &list(v.iter().map(ct_value))),
        CtValue::List(v) => positional("ct_list", &list(v.iter().map(ct_value))),
        CtValue::Dtype(v) => positional("ct_dtype", v.name()),
        CtValue::Simd { dtype, lanes } => record(
            "ct_simd",
            &[
                ("dtype", dtype.name().into()),
                (
                    "lanes",
                    list(lanes.iter().map(|lane| match lane {
                        mojito_types::ct::CtLane::Int(v) => positional("lane_int", &v.to_string()),
                        mojito_types::ct::CtLane::Float(v) => {
                            positional("lane_float_bits", &format!("{v:016x}"))
                        }
                        mojito_types::ct::CtLane::Bool(v) => {
                            positional("lane_bool", &v.to_string())
                        }
                    })),
                ),
            ],
        ),
        CtValue::Struct { name, fields } => record(
            "ct_struct",
            &[
                ("name", symbol(name)),
                (
                    "fields",
                    list(fields.iter().map(|(name, value)| {
                        record(
                            "ct_field",
                            &[("name", symbol(name)), ("value", ct_value(value))],
                        )
                    })),
                ),
            ],
        ),
        CtValue::Variant {
            alternatives,
            index,
            payload,
        } => {
            let mut fields = vec![
                ("alternatives", list(alternatives.iter().map(ty_value))),
                ("index", index.to_string()),
            ];
            if let Some(payload) = payload {
                fields.push(("payload", ct_value(payload)));
            }
            record("ct_variant", &fields)
        }
        CtValue::Dict { spelling, entries } => {
            let mut fields = Vec::new();
            if let Some(ty) = spelling {
                fields.push(("spelling", ty_value(ty)));
            }
            fields.push((
                "entries",
                list(entries.iter().map(|(key, value)| {
                    record(
                        "ct_entry",
                        &[("key", ct_value(key)), ("value", ct_value(value))],
                    )
                })),
            ));
            record("ct_dict", &fields)
        }
        CtValue::Set { spelling, elements } => {
            let mut fields = Vec::new();
            if let Some(ty) = spelling {
                fields.push(("spelling", ty_value(ty)));
            }
            fields.push(("elements", list(elements.iter().map(ct_value))));
            record("ct_set", &fields)
        }
        CtValue::Type(v) => positional("ct_type", &ty_value(v)),
        CtValue::Trait(traits) => positional("ct_trait", &list(traits.iter().map(|v| symbol(v)))),
        CtValue::KgenString(v) => positional("ct_kgen_string", &quote(v)),
        CtValue::MlirAttr(v) => positional("ct_mlir_attr", &quote(v)),
        CtValue::Generator(generator) => record(
            "ct_generator",
            &[
                ("params", list(generator.params.iter().map(param_decl))),
                ("body", param_expr(&generator.body)),
            ],
        ),
        CtValue::Expr(v) => positional("ct_expr", &param_expr(v)),
        CtValue::Deferred(v) => positional("ct_deferred", &binder_ref(v)),
        CtValue::Marker(v) => positional("ct_marker", &ct_marker(v)),
        CtValue::RemovedArg => "ct_removed_arg".into(),
    }
}

fn ct_marker(value: &CtMarker) -> String {
    match value {
        CtMarker::RuntimeLocal => "marker_local".into(),
        CtMarker::TypeName => "marker_type".into(),
        CtMarker::TupleOrigin { id, mutability } => record(
            "marker_tuple_origin",
            &[
                ("id", id.0.to_string()),
                ("mutability", mutability_value(*mutability)),
            ],
        ),
        CtMarker::Applied => "marker_applied".into(),
    }
}

fn origin_seg(value: &OriginSeg) -> String {
    match value {
        OriginSeg::Field(v) => positional("field", &symbol(v)),
        OriginSeg::AnyIndex => "any_index".into(),
        OriginSeg::Interior(v) => positional("interior", &symbol(v)),
        OriginSeg::Subtree => "subtree".into(),
    }
}
fn origin(value: &Origin) -> String {
    match value {
        Origin::Param(v) => positional("origin_param", &v.0.to_string()),
        Origin::SelfParam => "origin_self".into(),
        Origin::Place(v) => record(
            "origin_place",
            &[
                ("root", format!("$v{}", v.root.0)),
                ("path", list(v.path.iter().map(origin_seg))),
            ],
        ),
        Origin::Union(v) => positional("origin_union", &list(v.iter().map(origin))),
        Origin::Static => "origin_static".into(),
        Origin::Untracked { mutable } => {
            record("origin_untracked", &[("mutable", mutable.to_string())])
        }
        Origin::Unbound => "origin_unbound".into(),
    }
}
fn pointer_origin(value: &PointerOrigin) -> String {
    match value {
        PointerOrigin::Place { place, mutable } => record(
            "pointer_place",
            &[
                (
                    "place",
                    record(
                        "origin_place",
                        &[
                            ("root", format!("$v{}", place.root.0)),
                            ("path", list(place.path.iter().map(origin_seg))),
                        ],
                    ),
                ),
                ("mutable", mutable.to_string()),
            ],
        ),
        PointerOrigin::Param {
            id,
            mutability,
            interior,
            subtree,
        } => record(
            "pointer_param",
            &[
                ("id", id.0.to_string()),
                ("mutability", mutability_value(*mutability)),
                ("interior", list(interior.iter().map(|v| symbol(v)))),
                ("subtree", subtree.to_string()),
            ],
        ),
        PointerOrigin::SelfPlace {
            mutability,
            interior,
            subtree,
        } => record(
            "pointer_self",
            &[
                ("mutability", mutability_value(*mutability)),
                ("interior", list(interior.iter().map(|v| symbol(v)))),
                ("subtree", subtree.to_string()),
            ],
        ),
        PointerOrigin::Static => "pointer_static".into(),
        PointerOrigin::Untracked { mutable } => {
            record("pointer_untracked", &[("mutable", mutable.to_string())])
        }
        PointerOrigin::UnsafeAny { mutable } => {
            record("pointer_unsafe_any", &[("mutable", mutable.to_string())])
        }
    }
}
fn ref_ty(value: &RefTy) -> String {
    record(
        "ref",
        &[
            ("referent", ty_value(&value.referent)),
            ("origin", origin(&value.origin)),
            ("mutability", mutability_value(value.mutability)),
        ],
    )
}
fn mutability_value(value: Mutability) -> String {
    match value {
        Mutability::Immutable => "immutable".into(),
        Mutability::Mutable => "mutable".into(),
        Mutability::Param(v) => positional("mutability_param", &v.0.to_string()),
    }
}
fn ref_sig(value: &RefSig) -> String {
    record(
        "ref_sig",
        &[
            ("origin", sig_origin(&value.origin)),
            ("mutability", sig_mutability(&value.mutability)),
        ],
    )
}
fn sig_origin(value: &SigOrigin) -> String {
    match value {
        SigOrigin::Self_ => "sig_self".into(),
        SigOrigin::Param(v) => positional("sig_param", &v.to_string()),
        SigOrigin::Bound(v) => positional("sig_bound", &origin(v)),
        SigOrigin::Static => "sig_static".into(),
        SigOrigin::Untracked { mutable } => {
            record("sig_untracked", &[("mutable", mutable.to_string())])
        }
        SigOrigin::UnsafeAny { mutable } => {
            record("sig_unsafe_any", &[("mutable", mutable.to_string())])
        }
        SigOrigin::Projected(base, path) => record(
            "sig_projected",
            &[
                ("base", sig_origin(base)),
                ("path", list(path.iter().map(origin_seg))),
            ],
        ),
        SigOrigin::Union(v) => positional("sig_union", &list(v.iter().map(sig_origin))),
        SigOrigin::Infer => "sig_infer".into(),
        SigOrigin::Carried(ty) => positional("sig_carried", &ty_value(ty)),
    }
}
fn sig_mutability(value: &SigMutability) -> String {
    match value {
        SigMutability::Immutable => "sig_immutable".into(),
        SigMutability::Mutable => "sig_mutable".into(),
        SigMutability::BoolParam(v) => positional("sig_bool_param", &v.to_string()),
        SigMutability::Infer => "sig_infer".into(),
    }
}
fn environment_value(value: &CallableEnvironment) -> String {
    match value {
        CallableEnvironment::Default => "default".into(),
        CallableEnvironment::Thin => "thin".into(),
        CallableEnvironment::Capturing(v) => positional("capturing", &capture_set(v)),
        // A closure's checked type never reaches verified MIR; an
        // unverified dump spells its captures.
        CallableEnvironment::Storage(v) => positional("closure_storage", &capture_set(&v.captures)),
    }
}
fn capture_set(value: &CaptureOriginSet) -> String {
    match value {
        CaptureOriginSet::Infer => "capture_set_infer".into(),
        CaptureOriginSet::Param(v) => positional("capture_set_param", &v.0.to_string()),
        CaptureOriginSet::Concrete(v) => positional(
            "capture_set",
            &list(v.iter().map(|capture| {
                record(
                    "capture_origin",
                    &[
                        ("origin", origin(&capture.origin)),
                        ("access", capture_access_kind(capture.access).into()),
                    ],
                )
            })),
        ),
    }
}
const fn capture_access_kind(value: CaptureAccess) -> &'static str {
    match value {
        CaptureAccess::Read => "read",
        CaptureAccess::Write => "write",
    }
}

fn convention(value: ArgConvention) -> String {
    match value {
        ArgConvention::Imm => "imm",
        ArgConvention::Var => "var",
        ArgConvention::Mut => "mut",
        ArgConvention::Out => "out",
        ArgConvention::Ref => "ref",
        ArgConvention::Deinit => "deinit",
    }
    .into()
}
const fn prefix(value: PrefixOp) -> &'static str {
    match value {
        PrefixOp::Neg => "neg",
        PrefixOp::Not => "not",
        PrefixOp::Invert => "invert",
    }
}
const fn infix(value: InfixOp) -> &'static str {
    match value {
        InfixOp::Add => "add",
        InfixOp::Sub => "sub",
        InfixOp::Mul => "mul",
        InfixOp::Div => "div",
        InfixOp::FloorDiv => "floor_div",
        InfixOp::Mod => "mod",
        InfixOp::MatMul => "mat_mul",
        InfixOp::Shl => "shl",
        InfixOp::Shr => "shr",
        InfixOp::BitAnd => "bit_and",
        InfixOp::BitOr => "bit_or",
        InfixOp::BitXor => "bit_xor",
        InfixOp::Pow => "pow",
        InfixOp::Eq => "eq",
        InfixOp::Ne => "ne",
        InfixOp::Lt => "lt",
        InfixOp::Gt => "gt",
        InfixOp::Le => "le",
        InfixOp::Ge => "ge",
        InfixOp::And => "and",
        InfixOp::Or => "or",
        InfixOp::In => "in",
        InfixOp::NotIn => "not_in",
        InfixOp::Is => "is",
        InfixOp::IsNot => "is_not",
    }
}

fn symbol(value: &str) -> String {
    if super::is_bare_identifier(value) {
        value.into()
    } else {
        quote(value)
    }
}
fn reg_value(value: Reg) -> String {
    format!("%r{}", value.0)
}
fn var_value(value: mojito_hir::hir::VarId) -> String {
    format!("$v{value}")
}
fn regs(values: &[Reg]) -> String {
    list(values.iter().map(|v| reg_value(*v)))
}
fn kwargs_value(values: &[(String, Reg)]) -> String {
    list(values.iter().map(|(name, value)| {
        record(
            "keyword",
            &[("name", symbol(name)), ("value", reg_value(*value))],
        )
    }))
}
fn places_option(values: &[Option<MirPlace>]) -> String {
    list(values.iter().map(|v| option(v.as_ref().map(place_value))))
}
fn option(value: Option<String>) -> String {
    value.map_or_else(|| "absent".into(), |value| format!("present({value})"))
}
fn positional(tag: &str, value: &str) -> String {
    format!("{tag}({value})")
}
fn record(tag: &str, fields: &[(&str, String)]) -> String {
    if fields.is_empty() {
        format!("{tag} {{}}")
    } else {
        format!(
            "{tag} {{ {} }}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
fn list(values: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", values.into_iter().collect::<Vec<_>>().join(", "))
}
