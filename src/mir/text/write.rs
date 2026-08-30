use super::{instruction_mnemonic, quote, terminator_mnemonic, version_header};
use crate::ast::{ArgConvention, InfixOp, PrefixOp};
use crate::checked::{
    CheckedCallArgument, CheckedCallArgumentSource, CheckedConst, CheckedIteratorCall,
    CheckedResultAdapter,
};
use crate::ct::{CtExpr, CtValue};
use crate::mir::*;
use crate::origin::{
    CallableEnvironment, CaptureAccess, CaptureOriginSet, Mutability, Origin, OriginSeg,
    PointerOrigin, RefSig, RefTy, SigMutability, SigOrigin,
};
use crate::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, PackPredicateRef,
    ParamDecl, TrivialLifecycle, Ty, TyArg,
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
    writeln!(output, "{}", version_header()).unwrap();
    writeln!(output, "artifact {{").unwrap();
    writeln!(output, "  features: [],").unwrap();
    write_files(&mut output, &files);
    write_structs(&mut output, &program.declarations.structs);
    write_declarations(&mut output, &program.declarations.functions);
    writeln!(output, "  functions: [").unwrap();
    for (index, (name, function)) in program.functions.iter().enumerate() {
        write_function(&mut output, name, function, &files, 2);
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
    writeln!(output, "}}").unwrap();
    output
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
        let value = record(
            "struct",
            &[
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
                    option(
                        declaration
                            .explicit_destroy_message
                            .as_ref()
                            .map(|v| quote(v)),
                    ),
                ),
                ("explicit_destructors", destructors),
            ],
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
            ],
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
        MirInstr::CopyValue { dest, value } => record(
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
                            &[
                                ("place", place_value(&capture.place)),
                                (
                                    "mode",
                                    match capture.mode {
                                        MirCaptureMode::Reference => "reference",
                                        MirCaptureMode::Copy => "copy",
                                        MirCaptureMode::Move => "move",
                                    }
                                    .into(),
                                ),
                            ],
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
        MirInstr::SizeOf { dest, ty } => {
            record(tag, &[("dest", reg_value(*dest)), ("type", ty_value(ty))])
        }
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
        MirInstr::UseVar { dest, var, mode } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("var", var_value(*var)),
                ("mode", super::use_mode_spelling(*mode).into()),
            ],
        ),
        MirInstr::MovePlace { dest, place } => record(
            tag,
            &[("dest", reg_value(*dest)), ("place", place_value(place))],
        ),
        MirInstr::DefVar {
            var,
            src,
            binding_ty,
        } => record(
            tag,
            &[
                ("var", var_value(*var)),
                ("src", reg_value(*src)),
                ("binding_type", option(binding_ty.as_ref().map(ty_value))),
            ],
        ),
        MirInstr::UnOp { op, dest, a } => record(
            tag,
            &[
                ("op", prefix(*op).into()),
                ("dest", reg_value(*dest)),
                ("a", reg_value(*a)),
            ],
        ),
        MirInstr::BinOp {
            op,
            dest,
            a,
            b,
            resolved,
        } => record(
            tag,
            &[
                ("op", infix(*op).into()),
                ("dest", reg_value(*dest)),
                ("a", reg_value(*a)),
                ("b", reg_value(*b)),
                ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
            ],
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
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("func", symbol(&func.0)),
                ("raises", option(raises.as_ref().map(ty_value))),
                ("args", regs(args)),
                ("kwargs", kwargs_value(kwargs)),
                ("arg_places", places_option(arg_places)),
                ("kwarg_places", places_option(kwarg_places)),
                (
                    "capture_accesses",
                    list(capture_accesses.iter().map(capture_access)),
                ),
                ("param_arg_regs", list(param_arg_regs.iter().map(param_arg))),
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
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("callee", reg_value(*callee)),
                ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
                ("raises", option(raises.as_ref().map(ty_value))),
                ("args", regs(args)),
                ("kwargs", kwargs_value(kwargs)),
                (
                    "callee_place",
                    option(callee_place.as_ref().map(place_value)),
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
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("recv", reg_value(*recv)),
                ("method", symbol(method)),
                ("resolved", option(resolved.as_ref().map(|v| symbol(v)))),
                ("raises", option(raises.as_ref().map(ty_value))),
                (
                    "reference_result",
                    option(reference_result.as_ref().map(ref_ty)),
                ),
                (
                    "result_adapter",
                    option(result_adapter.map(result_adapter_value)),
                ),
                ("args", regs(args)),
                ("kwargs", kwargs_value(kwargs)),
                ("recv_place", option(recv_place.as_ref().map(place_value))),
                ("arg_places", places_option(arg_places)),
                ("kwarg_places", places_option(kwarg_places)),
                (
                    "capture_accesses",
                    list(capture_accesses.iter().map(capture_access)),
                ),
                ("param_arg_regs", list(param_arg_regs.iter().map(param_arg))),
                ("param_decls", list(param_decls.iter().map(param_decl))),
            ],
        ),
        MirInstr::PointerStorageTake {
            dest,
            pointer,
            index,
            element,
        }
        | MirInstr::PointerStorageDestroy {
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
        }
        | MirInstr::UninitStorageDestroy {
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
                ("base_place", option(base_place.as_ref().map(place_value))),
                ("index_place", option(index_place.as_ref().map(place_value))),
                ("call", option(call.as_ref().map(subscript_call))),
                (
                    "intrinsic",
                    option(intrinsic.map(|v| super::intrinsic_subscript_spelling(v).into())),
                ),
            ],
        ),
        MirInstr::Slice {
            dest,
            object,
            kind,
            lower,
            upper,
            step,
            object_place,
            arg_places,
            call,
            intrinsic,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("object", reg_value(*object)),
                ("kind", slice_kind(*kind).into()),
                ("lower", option(lower.map(reg_value))),
                ("upper", option(upper.map(reg_value))),
                ("step", option(step.map(reg_value))),
                (
                    "object_place",
                    option(object_place.as_ref().map(place_value)),
                ),
                ("arg_places", places_option(arg_places)),
                ("call", option(call.as_ref().map(subscript_call))),
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
                    option(object_place.as_ref().map(place_value)),
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
                ("call", option(call.as_ref().map(subscript_call))),
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
                    option(receiver_place.as_ref().map(place_value)),
                ),
                ("args", list(args.iter().map(subscript_arg))),
                ("arg_places", places_option(arg_places)),
                ("value", reg_value(*value)),
                ("value_place", option(value_place.as_ref().map(place_value))),
                ("value_keyword", value_keyword.to_string()),
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
        MirInstr::MakeVariant {
            dest,
            alternatives,
            index,
            value,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("alternatives", list(alternatives.iter().map(ty_value))),
                ("index", index.to_string()),
                ("value", reg_value(*value)),
            ],
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
                ("index", index.to_string()),
            ],
        ),
        MirInstr::VariantSet {
            dest,
            place,
            index,
            value,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("place", place_value(place)),
                ("index", index.to_string()),
                ("value", reg_value(*value)),
            ],
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
                ("index", index.to_string()),
                ("checked", checked.to_string()),
            ],
        ),
        MirInstr::VariantSetInitWith {
            dest,
            place,
            index,
            factory,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("place", place_value(place)),
                ("index", index.to_string()),
                ("factory", reg_value(*factory)),
            ],
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
                ("index", index.to_string()),
            ],
        ),
        MirInstr::VariantReplace {
            dest,
            place,
            input_index,
            output_index,
            value,
            checked,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("place", place_value(place)),
                ("input_index", input_index.to_string()),
                ("output_index", output_index.to_string()),
                ("value", reg_value(*value)),
                ("checked", checked.to_string()),
            ],
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
                ("dtype", dtype.name().into()),
                ("width", width.to_string()),
                ("elems", regs(elems)),
            ],
        ),
        MirInstr::SimdCast {
            dest,
            value,
            dtype,
            width,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("dtype", dtype.name().into()),
                ("width", width.to_string()),
            ],
        ),
        MirInstr::SimdShuffle { dest, value, mask } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("value", reg_value(*value)),
                ("mask", list(mask.iter().map(usize::to_string))),
            ],
        ),
        MirInstr::Raise { src } => record(tag, &[("src", reg_value(*src))]),
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } => record(
            tag,
            &[
                ("body", blocks_value(body)),
                (
                    "handler",
                    option(handler.as_ref().map(|(var, blocks)| {
                        record(
                            "handler",
                            &[
                                ("error_var", option(var.map(var_value))),
                                ("blocks", blocks_value(blocks)),
                            ],
                        )
                    })),
                ),
                ("orelse", option(orelse.as_ref().map(|v| blocks_value(v)))),
                (
                    "finalbody",
                    option(finalbody.as_ref().map(|v| blocks_value(v))),
                ),
                ("cleanup", list(cleanup.iter().map(|v| var_value(*v)))),
            ],
        ),
        MirInstr::Drop { reg } => record(tag, &[("reg", reg_value(*reg))]),
        MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            record(tag, &[("var", var_value(*var))])
        }
        MirInstr::ConsumePlace { place, marker } => record(
            tag,
            &[
                ("place", place_value(place)),
                ("marker", reg_value(*marker)),
            ],
        ),
        MirInstr::Unsupported(message) => record(tag, &[("message", quote(message))]),
        MirInstr::GetIter {
            source,
            dest,
            mode,
            prepare,
        } => record(
            tag,
            &[
                ("source", var_value(*source)),
                ("dest", var_value(*dest)),
                (
                    "mode",
                    match mode {
                        crate::IterationMode::Borrowed => "borrowed",
                        crate::IterationMode::Owned => "owned",
                    }
                    .into(),
                ),
                ("prepare", list(prepare.iter().map(|v| symbol(v)))),
            ],
        ),
        MirInstr::HasNext { dest, iter, method } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("iter", var_value(*iter)),
                ("method", option(method.as_ref().map(|v| symbol(v)))),
            ],
        ),
        MirInstr::Next { dest, iter, call } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("iter", var_value(*iter)),
                ("call", option(call.as_ref().map(iterator_call))),
            ],
        ),
        MirInstr::TryNext {
            dest,
            yielded,
            iter,
            call,
            exhaustion,
        } => record(
            tag,
            &[
                ("dest", reg_value(*dest)),
                ("yielded", reg_value(*yielded)),
                ("iter", var_value(*iter)),
                ("call", iterator_call(call)),
                ("exhaustion", ty_value(exhaustion)),
            ],
        ),
    }
}

fn term_value(term: &MirTerm) -> String {
    let tag = terminator_mnemonic(term);
    match term {
        MirTerm::Jump(target) => record(tag, &[("target", format!("bb{target}"))]),
        MirTerm::Branch {
            cond,
            then_b,
            else_b,
        } => record(
            tag,
            &[
                ("condition", reg_value(*cond)),
                ("then", format!("bb{then_b}")),
                ("else", format!("bb{else_b}")),
            ],
        ),
        MirTerm::Return(value) => record(tag, &[("value", option(value.map(reg_value)))]),
        MirTerm::ReturnWithCleanup { value, cleanup } => record(
            tag,
            &[
                ("value", option(value.map(reg_value))),
                ("cleanup", list(cleanup.iter().map(|v| var_value(*v)))),
            ],
        ),
        MirTerm::FallOff => record(tag, &[]),
        MirTerm::EscapeJump { target, cleanup } => record(
            tag,
            &[
                ("target", format!("bb{target}")),
                ("cleanup", list(cleanup.iter().map(|v| var_value(*v)))),
            ],
        ),
    }
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
        Proj::Field(v) => positional("field", symbol(v)),
        Proj::Index(v) => positional("index", reg_value(*v)),
        Proj::ConstIndex(v) => positional("const_index", v.to_string()),
        Proj::Variant(v) => positional("variant", v.to_string()),
        Proj::UninitPayload => "uninit_payload".into(),
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
fn param_arg(value: &MirParamArg) -> String {
    record(
        "param_arg",
        &[
            ("name", option(value.name.as_ref().map(|v| symbol(v)))),
            ("value", option(value.value.map(reg_value))),
        ],
    )
}
fn subscript_arg(value: &MirSubscriptArg) -> String {
    match value {
        MirSubscriptArg::Index(v) => positional("index", reg_value(*v)),
        MirSubscriptArg::Slice {
            kind,
            lower,
            upper,
            step,
        } => record(
            "slice_arg",
            &[
                ("kind", slice_kind(*kind).into()),
                ("lower", option(lower.map(reg_value))),
                ("upper", option(upper.map(reg_value))),
                ("step", option(step.map(reg_value))),
            ],
        ),
    }
}

fn subscript_call(value: &MirSubscriptCall) -> String {
    record(
        "subscript_call",
        &[
            ("target", symbol(&value.target)),
            ("raises", option(value.raises.as_ref().map(ty_value))),
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
                        positional("positional", v.to_string())
                    }
                    CheckedCallArgumentSource::Keyword(v) => positional("keyword", v.to_string()),
                    CheckedCallArgumentSource::Default => "default".into(),
                },
            ),
            ("parameter_type", ty_value(&value.parameter_ty)),
            ("requires_place", value.requires_place.to_string()),
            ("convention", option(value.convention.map(convention))),
        ],
    )
}
fn iterator_call(value: &CheckedIteratorCall) -> String {
    record(
        "iterator_call",
        &[
            ("target", symbol(&value.target)),
            ("result_type", ty_value(&value.result_ty)),
            (
                "reference_result",
                option(value.reference_result.as_ref().map(ref_ty)),
            ),
            ("raises", option(value.raises.as_ref().map(ty_value))),
            (
                "result_adapter",
                option(value.result_adapter.map(result_adapter_value)),
            ),
        ],
    )
}
fn result_adapter_value(value: CheckedResultAdapter) -> String {
    match value {
        CheckedResultAdapter::CopyIteratorReference => "copy_iterator_reference".into(),
    }
}

fn ty_value(ty: &Ty) -> String {
    match ty {
        Ty::Int => "Int".into(),
        Ty::UInt => "UInt".into(),
        Ty::Bool => "Bool".into(),
        Ty::StringLiteral => "StringLiteral".into(),
        Ty::Float64 => "Float64".into(),
        Ty::None => "None".into(),
        Ty::Never => "Never".into(),
        Ty::IntLiteral => "IntLiteral".into(),
        Ty::FloatLiteral => "FloatLiteral".into(),
        Ty::Infer => "Infer".into(),
        Ty::Dtype => "DType".into(),
        Ty::SelfType => "Self".into(),
        Ty::Error => "Error".into(),
        Ty::Func {
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
        } => callable_type(
            "func",
            environment,
            &[],
            params,
            names,
            ret,
            required,
            variadic.as_deref(),
            kw_variadic.as_deref(),
            *positional_only,
            *keyword_only,
            *raises,
            error.as_deref(),
            conventions,
            ref_params,
            ref_return.as_deref(),
            transfers,
        ),
        Ty::GenericFunc {
            environment,
            decls,
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
        } => callable_type(
            "generic_func",
            environment,
            decls,
            params,
            names,
            ret,
            required,
            variadic.as_deref(),
            kw_variadic.as_deref(),
            *positional_only,
            *keyword_only,
            *raises,
            error.as_deref(),
            conventions,
            ref_params,
            ref_return.as_deref(),
            transfers,
        ),
        Ty::Overload(values) => positional("overload", list(values.iter().map(ty_value))),
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => record(
            "param",
            &[
                ("name", symbol(name)),
                ("bounds", list(bounds.iter().map(|v| symbol(v)))),
                (
                    "callable_bound",
                    option(callable_bound.as_deref().map(ty_value)),
                ),
            ],
        ),
        Ty::Assoc { base, name, args } => record(
            "assoc",
            &[
                ("base", ty_value(base)),
                ("member", symbol(name)),
                ("arguments", list(args.iter().map(ty_arg))),
            ],
        ),
        Ty::Dependent(DependentType::Indexed { elements, index }) => record(
            "dependent_index",
            &[
                ("elements", list(elements.iter().map(ty_value))),
                ("index", ct_expr(index)),
            ],
        ),
        Ty::Struct(name, args) => record(
            "struct_type",
            &[
                ("name", symbol(name)),
                ("arguments", list(args.iter().map(ty_arg))),
            ],
        ),
        Ty::Simd { dtype, width } => record(
            "simd",
            &[("dtype", dtype.name().into()), ("width", width.to_string())],
        ),
        Ty::ComptimeList(value) => positional("comptime_list", ty_value(value)),
        Ty::Tuple(values) => positional("tuple", list(values.iter().map(ty_value))),
        Ty::RuntimePack(values) => positional("runtime_pack", list(values.iter().map(ty_value))),
        Ty::VariadicPack(value) => positional("variadic_pack", ty_value(value)),
        Ty::Variant(values) => positional("variant", list(values.iter().map(ty_value))),
        Ty::Pointer { element, origin } => record(
            "pointer",
            &[
                ("element", ty_value(element)),
                ("origin", pointer_origin(origin)),
            ],
        ),
        Ty::Ref(value) => ref_ty(value),
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
    transfers: &crate::checked::TransferSet,
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

fn transfer_set(value: &crate::checked::TransferSet) -> String {
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
        TyArg::Ty(v) => positional("type_arg", ty_value(v)),
        TyArg::Val(v) => positional("value_arg", ct_value(v)),
        TyArg::Origin(v) => positional("origin_arg", origin(v)),
    }
}
fn param_decl(value: &ParamDecl) -> String {
    match value {
        ParamDecl::Type {
            name,
            bounds,
            callable_bound,
            default,
            infer_only,
            variadic,
            constraints,
        } => record(
            "type_param",
            &[
                ("name", symbol(name)),
                ("bounds", list(bounds.iter().map(|v| symbol(v)))),
                (
                    "callable_bound",
                    option(callable_bound.as_deref().map(ty_value)),
                ),
                ("default", option(default.as_deref().map(ty_value))),
                ("infer_only", infer_only.to_string()),
                ("variadic", variadic.to_string()),
                ("constraints", list(constraints.iter().map(constraint))),
            ],
        ),
        ParamDecl::Value {
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
                ("name", symbol(name)),
                ("type", ty_value(ty)),
                ("default", option(default.as_ref().map(ct_expr))),
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
        GenericConstraint::Conforms { param, trait_name } => record(
            "conforms",
            &[("param", symbol(param)), ("trait", symbol(trait_name))],
        ),
        GenericConstraint::ConformsPack { param, trait_name } => record(
            "conforms_pack",
            &[("param", symbol(param)), ("trait", symbol(trait_name))],
        ),
        GenericConstraint::PackPredicate {
            param,
            predicate,
            all,
        } => record(
            "pack_predicate",
            &[
                ("param", symbol(param)),
                ("predicate", pack_predicate(predicate)),
                ("all", all.to_string()),
            ],
        ),
        GenericConstraint::PackContains { param, element } => record(
            "pack_contains",
            &[
                ("param", symbol(param)),
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
        GenericConstraint::Not(value) => positional("not", constraint(value)),
        GenericConstraint::Bool(value) => positional("constraint_bool", value.to_string()),
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
        ConstraintOperand::Param(value) => positional("operand_param", symbol(value)),
        ConstraintOperand::Value(value) => positional("operand_value", ct_value(value)),
        ConstraintOperand::Type(value) => positional("operand_type", ty_value(value)),
        ConstraintOperand::PackLength(value) => positional("operand_pack_length", symbol(value)),
    }
}

fn pack_predicate(value: &PackPredicateRef) -> String {
    match value {
        PackPredicateRef::Trivial(value) => {
            positional("predicate_trivial", lifecycle(*value).into())
        }
        PackPredicateRef::Alias(value) => positional("predicate_alias", symbol(value)),
    }
}

fn lifecycle(value: TrivialLifecycle) -> &'static str {
    match value {
        TrivialLifecycle::Movable => "movable",
        TrivialLifecycle::Copyable => "copyable",
        TrivialLifecycle::Deinitable => "deinitable",
    }
}

fn callable_default_value(value: &CallableDefault) -> String {
    match value {
        CallableDefault::Symbol(v) => positional("default_symbol", symbol(v)),
        CallableDefault::Parameter(v) => positional("default_parameter", symbol(v)),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => record(
            "default_if",
            &[
                ("condition", ct_expr(condition)),
                ("then_value", callable_default_value(then_value)),
                ("else_value", callable_default_value(else_value)),
            ],
        ),
    }
}

fn const_value(value: &Const) -> String {
    match value {
        Const::Int(v) => positional("int", v.to_string()),
        Const::Float(v) => positional("float", format!("{:016x}", v.to_bits())),
        Const::IntLiteral(v) => positional("int_literal", v.to_string()),
        Const::FloatLiteral(v) => positional("float_literal", v.to_string()),
        Const::Bool(v) => positional("bool", v.to_string()),
        Const::Str(v) => positional("string", quote(v)),
        Const::Function(v) => positional("function", symbol(v)),
        Const::None => "none".into(),
    }
}
fn checked_const(value: &CheckedConst) -> String {
    match value {
        CheckedConst::Int(v) => positional("checked_int", v.to_string()),
        CheckedConst::Float(v) => positional("checked_float", v.to_string()),
        CheckedConst::Bool(v) => positional("checked_bool", v.to_string()),
        CheckedConst::String(v) => positional("checked_string", quote(v)),
        CheckedConst::None => "checked_none".into(),
        CheckedConst::Construct { target, arg } => record(
            "checked_construct",
            &[("target", quote(target)), ("arg", checked_const(arg))],
        ),
    }
}
fn ct_expr(value: &CtExpr) -> String {
    match value {
        CtExpr::Value(v) => positional("ct_value", ct_value(v)),
        CtExpr::Param(v) => positional("ct_param", symbol(v)),
        CtExpr::Neg(v) => positional("ct_neg", ct_expr(v)),
        CtExpr::Add(a, b) => binary("ct_add", a, b),
        CtExpr::Sub(a, b) => binary("ct_sub", a, b),
        CtExpr::Mul(a, b) => binary("ct_mul", a, b),
        CtExpr::FloorDiv(a, b) => binary("ct_floor_div", a, b),
        CtExpr::Mod(a, b) => binary("ct_mod", a, b),
        CtExpr::Pow(a, b) => binary("ct_pow", a, b),
    }
}
fn binary(tag: &str, left: &CtExpr, right: &CtExpr) -> String {
    record(tag, &[("left", ct_expr(left)), ("right", ct_expr(right))])
}
fn ct_value(value: &CtValue) -> String {
    match value {
        CtValue::Int(v) => positional("ct_int", v.to_string()),
        CtValue::UInt(v) => positional("ct_uint", v.to_string()),
        CtValue::Float(v) => positional("ct_float_bits", format!("{v:016x}")),
        CtValue::IntLiteral(v) => positional("ct_int_literal", v.to_string()),
        CtValue::FloatLiteral(v) => positional("ct_float_literal", v.to_string()),
        CtValue::Bool(v) => positional("ct_bool", v.to_string()),
        CtValue::Str(v) => positional("ct_string", quote(v)),
        CtValue::Tuple(v) => positional("ct_tuple", list(v.iter().map(ct_value))),
        CtValue::List(v) => positional("ct_list", list(v.iter().map(ct_value))),
        CtValue::Dtype(v) => positional("ct_dtype", v.name().into()),
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
        CtValue::Type(v) => positional("ct_type", ty_value(v)),
        CtValue::Reflected(v) => positional("ct_reflected", ty_value(v)),
        CtValue::Param(v) => positional("ct_param", symbol(v)),
    }
}

fn origin_seg(value: &OriginSeg) -> String {
    match value {
        OriginSeg::Field(v) => positional("field", symbol(v)),
        OriginSeg::AnyIndex => "any_index".into(),
        OriginSeg::Interior(v) => positional("interior", symbol(v)),
        OriginSeg::Subtree => "subtree".into(),
    }
}
fn origin(value: &Origin) -> String {
    match value {
        Origin::Param(v) => positional("origin_param", v.0.to_string()),
        Origin::SelfParam => "origin_self".into(),
        Origin::Place(v) => record(
            "origin_place",
            &[
                ("root", format!("$v{}", v.root.0)),
                ("path", list(v.path.iter().map(origin_seg))),
            ],
        ),
        Origin::Union(v) => positional("origin_union", list(v.iter().map(origin))),
        Origin::Static => "origin_static".into(),
        Origin::Untracked { mutable } => {
            record("origin_untracked", &[("mutable", mutable.to_string())])
        }
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
        Mutability::Param(v) => positional("mutability_param", v.0.to_string()),
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
        SigOrigin::Param(v) => positional("sig_param", v.to_string()),
        SigOrigin::Bound(v) => positional("sig_bound", origin(v)),
        SigOrigin::Static => "sig_static".into(),
        SigOrigin::Untracked { mutable } => {
            record("sig_untracked", &[("mutable", mutable.to_string())])
        }
        SigOrigin::Projected(base, path) => record(
            "sig_projected",
            &[
                ("base", sig_origin(base)),
                ("path", list(path.iter().map(origin_seg))),
            ],
        ),
        SigOrigin::Union(v) => positional("sig_union", list(v.iter().map(sig_origin))),
        SigOrigin::Infer => "sig_infer".into(),
    }
}
fn sig_mutability(value: &SigMutability) -> String {
    match value {
        SigMutability::Immutable => "sig_immutable".into(),
        SigMutability::Mutable => "sig_mutable".into(),
        SigMutability::BoolParam(v) => positional("sig_bool_param", v.to_string()),
        SigMutability::Infer => "sig_infer".into(),
    }
}
fn environment_value(value: &CallableEnvironment) -> String {
    match value {
        CallableEnvironment::Default => "default".into(),
        CallableEnvironment::Thin => "thin".into(),
        CallableEnvironment::Capturing(v) => positional("capturing", capture_set(v)),
    }
}
fn capture_set(value: &CaptureOriginSet) -> String {
    match value {
        CaptureOriginSet::Infer => "capture_set_infer".into(),
        CaptureOriginSet::Param(v) => positional("capture_set_param", v.0.to_string()),
        CaptureOriginSet::Concrete(v) => positional(
            "capture_set",
            list(v.iter().map(|capture| {
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
fn capture_access_kind(value: CaptureAccess) -> &'static str {
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
fn prefix(value: PrefixOp) -> &'static str {
    match value {
        PrefixOp::Neg => "neg",
        PrefixOp::Not => "not",
    }
}
fn infix(value: InfixOp) -> &'static str {
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
    }
}
fn slice_kind(value: crate::types::SliceKind) -> &'static str {
    match value {
        crate::types::SliceKind::Slice => "slice",
        crate::types::SliceKind::ContiguousSlice => "contiguous_slice",
        crate::types::SliceKind::StridedSlice => "strided_slice",
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
fn var_value(value: crate::hir::VarId) -> String {
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
fn positional(tag: &str, value: String) -> String {
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
