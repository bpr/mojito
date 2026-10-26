//! The binder scope and kind of every parametric type a function names.
//!
//! Parametric MIR names a type parameter (`TyKind::Param`), a dependent type, a
//! symbolic vector slot, and a symbolic struct argument through the binders
//! of the declaration that owns the body. A binder is in scope when the
//! function's own declaration or, for a method, its struct declares it, when
//! a generic callable signature enclosing the occurrence declares it, or when
//! it is a contract binder: one spelled by the callable contract or witness
//! request that holds it (`$contract`, `$callable`, `$synthetic:…`). Each
//! expression is well-kinded for its slot: an `Int` lane count, a `DType`
//! lane, a `Type` dependent type, a declared value parameter's type in a
//! struct argument, and a declared binder's own kind at every reference. The
//! rule is the one `docs/notes/param-expr-attributes.md` §Register types
//! records; the concrete mode rejects every such type instead.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::Const;
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{MetaTy, ParamExpr, ParamId, ParamKind};
use mojito_types::types::TyKind;
use mojito_types::types::{
    ConstraintOperand, DependentType, FuncTy, GenericConstraint, GenericFuncTy, SimdDtype,
    SimdWidth,
};

/// The scope and kind findings of one function body.
pub(super) fn verify_scope(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    let head = format!("MIR function '{name}'");
    // A forward to a callee's removed argument is no use: upstream's
    // `RemoveUnusedParams` rewrote the call without it. The body reads as
    // that rewrite leaves it, for the callees its snapshot names.
    let arguments_view = (!function.callee_removed_args.is_empty())
        .then(|| {
            crate::mir::without_removed_argument_forwards(function, declarations, &|callee| {
                function
                    .callee_removed_args
                    .iter()
                    .find(|(name, _)| name == callee)
                    .map(|(_, slots)| slots.as_slice())
            })
        })
        .flatten();
    let function = arguments_view.as_ref().map_or(function, |(view, _)| view);
    // A removed argument no longer exists: the body reads as the generator
    // a direct call names, with the argument gone, and uses its slot only
    // to drop a value whose drop runs no code.
    let declaration = declarations.function(name);
    let reduced = match declaration {
        Some(declaration) if !function.removed_args.is_empty() => {
            let used = crate::mir::used_slots(function, declarations);
            let reported = errors.len();
            for &slot in &function.removed_args {
                if slot as usize >= function.n_params {
                    errors.push(format!(
                        "{head} removes argument slot {slot}, which is no parameter"
                    ));
                } else if used.contains(&slot) {
                    errors.push(format!(
                        "{head} removes argument slot {slot}, which it uses"
                    ));
                }
            }
            if errors.len() > reported {
                return;
            }
            Some(crate::mir::without_removed_arguments(
                function,
                declaration,
                &function.removed_args,
            ))
        }
        _ => None,
    };
    let (function, declaration) = match &reduced {
        Some((view, declaration)) => (view, Some(declaration)),
        None => (function, declaration),
    };
    // A forward to a callee's removed parameter is no read: upstream's
    // `RemoveUnusedParams` rewrote the call without it. The body reads as
    // that rewrite leaves it, for the callees its snapshot names, and a
    // nested body it names reads what that body's snapshotted removals keep.
    let snapshot = |callee: &str| {
        function
            .callee_removed_params
            .iter()
            .find(|(name, _)| name == callee)
            .map(|(_, ids)| ids.as_slice())
    };
    let view = (!function.removed_params.is_empty())
        .then(|| crate::mir::without_removed_forwards(name, function, declarations, &snapshot))
        .flatten();
    let function = view.as_ref().map_or(function, |(view, _)| view);
    let mut scope = Scope::of_function(name, declarations);
    scope.declare_loop_binders(&function.blocks);
    // A removed parameter no longer exists: a reference to it is out of
    // scope but inside a struct argument, which an instance erases
    // (`_REMOVED_ARG`) as upstream's generator spells it, and its slot is
    // read by nothing.
    if !function.removed_params.is_empty() {
        let unread = declaration
            .map(|declaration| {
                crate::mir::unread_parameters_of(
                    name,
                    function,
                    declaration,
                    declarations,
                    &snapshot,
                    &snapshot,
                )
                .params
            })
            .unwrap_or_default();
        for id in &function.removed_params {
            if let Some(kind) = scope.kinds.remove(id) {
                scope.erased.insert(id.clone(), kind);
            }
            if !unread.contains(id) {
                errors.push(format!(
                    "{head} removes parameter slot {} of `{}`, which it reads",
                    id.slot, id.owner
                ));
            }
        }
    }
    let mut cx = ScopeCx {
        scope: &scope,
        declarations,
        head: &head,
        errors,
        erasable: 0,
        callee_removed_params: &function.callee_removed_params,
    };
    let signature = function
        .param_types
        .iter()
        .map(|ty| ("parameter type".to_string(), ty))
        .chain(
            function
                .ret_ty
                .iter()
                .map(|ty| ("return type".to_string(), ty)),
        )
        .chain(
            function
                .error_ty
                .iter()
                .map(|ty| ("error type".to_string(), ty)),
        );
    for (role, ty) in signature {
        cx.walk(&role, ty);
    }
    let mut slots: Vec<_> = function.var_tys.iter().collect();
    slots.sort_by_key(|(slot, _)| **slot);
    for (slot, ty) in slots {
        cx.walk(&format!("variable slot {slot}"), ty);
    }
    let mut registers: Vec<_> = function.reg_types.iter().collect();
    registers.sort_by_key(|(register, _)| **register);
    for (register, ty) in registers {
        cx.walk(&format!("register r{register}"), ty);
    }
    cx.blocks(&function.blocks);
}

/// The types an instruction names outside its places and registers, each
/// with the role the finding spells.
pub fn instruction_named_types(instruction: &MirInstr) -> Vec<(&'static str, &Ty)> {
    let mut types = Vec::new();
    match instruction {
        MirInstr::SizeOf { ty, .. }
        | MirInstr::MaterializeLiteral { target: ty, .. }
        | MirInstr::PointerStorageTake { element: ty, .. }
        | MirInstr::PointerStorageDestroy { element: ty, .. }
        | MirInstr::UninitStorageTake { element: ty, .. }
        | MirInstr::UninitStorageDestroy { element: ty, .. }
        | MirInstr::DefVar {
            binding_ty: Some(ty),
            ..
        } => types.push(("instruction", ty)),
        MirInstr::TypeName { ty, .. } => types.push(("type name", ty)),
        MirInstr::ConstructType { ty, .. } => types.push(("constructed type", ty)),
        MirInstr::Call {
            raises,
            receiver,
            instantiated_args,
            ..
        } => {
            types.extend(
                raises
                    .as_ref()
                    .map(|raises| &raises.ty)
                    .into_iter()
                    .chain(receiver.as_ref())
                    .map(|ty| ("instruction", ty)),
            );
            types.extend(
                instantiated_args
                    .iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(ty) => Some(("instantiated argument", ty)),
                        _ => None,
                    }),
            );
        }
        MirInstr::MakeTuple { element_types, .. } => {
            types.extend(
                element_types
                    .iter()
                    .flatten()
                    .map(|ty| ("tuple element", ty)),
            );
        }
        MirInstr::MakeVariant { alternatives, .. } => {
            types.extend(alternatives.iter().map(|ty| ("variant alternative", ty)));
        }
        MirInstr::CallIndirect {
            raises,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            types.extend(
                raises
                    .as_ref()
                    .map(|raises| &raises.ty)
                    .into_iter()
                    .chain(instantiated_contract.as_ref())
                    .map(|ty| ("callable contract", ty)),
            );
            types.extend(
                instantiated_args
                    .iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(ty) => Some(("callable argument", ty)),
                        _ => None,
                    }),
            );
        }
        MirInstr::MethodCall {
            raises,
            instantiated_args,
            ..
        } => {
            types.extend(raises.as_ref().map(|raises| ("error contract", &raises.ty)));
            types.extend(
                instantiated_args
                    .iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(ty) => Some(("instantiated argument", ty)),
                        _ => None,
                    }),
            );
        }
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => {
            types.extend(
                call.raises
                    .iter()
                    .map(|raises| &raises.ty)
                    .chain(std::iter::once(&call.result_ty))
                    .map(|ty| ("subscript contract", ty)),
            );
        }
        _ => {}
    }
    types
}

/// The alternatives a `Variant` operation selects, outside its places.
pub fn instruction_variant_indices(
    instruction: &MirInstr,
) -> Vec<&mojito_types::types::VariantIndex> {
    match instruction {
        MirInstr::MakeVariant { index, .. }
        | MirInstr::VariantIs { index, .. }
        | MirInstr::VariantGet { index, .. }
        | MirInstr::VariantSet { index, .. }
        | MirInstr::VariantTake { index, .. }
        | MirInstr::VariantSetInitWith { index, .. }
        | MirInstr::VariantDeinitWith { index, .. } => vec![index],
        MirInstr::VariantReplace {
            input_index,
            output_index,
            ..
        } => vec![input_index, output_index],
        _ => Vec::new(),
    }
}

/// Whether a binder is one a callable contract or witness request spells,
/// bound by the construct that holds it rather than by a declaration.
pub(super) fn is_contract_binder(id: &ParamId) -> bool {
    id.owner.starts_with('$')
}

/// The binders a body may name, with the kind each declares.
struct Scope {
    kinds: HashMap<ParamId, MetaTy>,
    /// The binders the body's generator removed, which only a struct
    /// argument may still name.
    erased: HashMap<ParamId, MetaTy>,
}

impl Scope {
    /// The binders of the function `name`, of each declaration it is nested
    /// in, of the regions each is declared in, and of the struct whose
    /// method the outermost one is.
    fn of_function(name: &str, declarations: &MirDeclarations) -> Self {
        let mut scope = Self {
            kinds: HashMap::new(),
            erased: HashMap::new(),
        };
        let mut link = Some(name);
        while let Some(current) = link {
            let declaration = declarations.function(current);
            if let Some(declaration) = declaration {
                scope.declare(&declaration.param_decls);
                scope.declare(&declaration.region_binders);
            }
            link = declaration.and_then(|declaration| declaration.enclosing.as_deref());
        }
        scope.declare(crate::mir::method_owner_params(name, declarations));
        scope
    }

    /// Bring every `comptime for` binder of `blocks` into scope: a loop's
    /// own binder, of its sequence's element type, declared by no signature,
    /// that the body's types and conditions name.
    fn declare_loop_binders(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                if let MirInstr::ComptimeFor { binder, source, .. } = instruction {
                    self.kinds.insert(binder.id.clone(), source.binder_meta());
                }
                for region in crate::mir::instruction_regions(instruction) {
                    self.declare_loop_binders(region);
                }
            }
        }
    }

    fn declare(&mut self, decls: &[ParamDecl]) {
        for decl in decls {
            self.kinds.insert(decl.id().clone(), decl.binder_meta());
        }
    }
}

struct ScopeCx<'a> {
    scope: &'a Scope,
    declarations: &'a MirDeclarations,
    head: &'a str,
    errors: &'a mut Vec<String>,
    /// How many places the walk is inside that may still spell a binder
    /// the body's generator removed: a struct argument, which an instance
    /// erases, or an application's argument for a binder the lifted body it
    /// applies removed, which upstream's `RemoveUnusedParams` rewrote the
    /// application without.
    erasable: usize,
    /// The body's snapshot of the removals of the callees it reads through.
    callee_removed_params: &'a [(String, Vec<ParamId>)],
}

impl ScopeCx<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for (index, block) in blocks.iter().enumerate() {
            for instruction in &block.instrs {
                let regions = crate::mir::instruction_regions(instruction);
                if !regions.is_empty() {
                    match instruction {
                        MirInstr::ComptimeIf { conds, .. } => {
                            for cond in conds {
                                self.constraint(&format!("block {index} compile-time if"), cond);
                            }
                        }
                        MirInstr::ComptimeFor { binder, source, .. } => {
                            self.comptime_loop(
                                &format!("block {index} compile-time loop"),
                                binder,
                                source,
                            );
                        }
                        _ => {}
                    }
                    for region in regions {
                        self.blocks(region);
                    }
                    continue;
                }
                let role = format!("block {index} place");
                for place in instruction_places(instruction) {
                    let types = place
                        .root_ty
                        .iter()
                        .chain(&place.projection_tys)
                        .chain(place.ty.iter());
                    for ty in types {
                        self.walk(&role, ty);
                    }
                }
                for (what, ty) in instruction_named_types(instruction) {
                    self.walk(&format!("block {index} {what}"), ty);
                }
                // A SIMD instruction's slots are the slots of the vector type
                // it builds, checked as that type is.
                if let MirInstr::MakeSimd { dtype, width, .. }
                | MirInstr::SimdCast { dtype, width, .. }
                | MirInstr::SimdBitcast { dtype, width, .. } = instruction
                {
                    let built = Ty::new(TyKind::Simd {
                        dtype: dtype.clone(),
                        width: width.clone(),
                    });
                    self.walk(&format!("block {index} vector slots"), &built);
                }
                let projected = instruction_places(instruction)
                    .into_iter()
                    .flat_map(|place| &place.proj)
                    .filter_map(|projection| match projection {
                        Proj::Variant(alternative) => Some(alternative),
                        _ => None,
                    });
                for alternative in instruction_variant_indices(instruction)
                    .into_iter()
                    .chain(projected)
                {
                    if let mojito_types::types::VariantIndex::Expr(value) = alternative {
                        let role = format!("block {index} variant alternative");
                        let mut nested = Vec::new();
                        self.expr_nodes(&role, "variant alternative", value, &mut nested);
                    }
                }
                if let MirInstr::SimdShuffle { mask, .. } = instruction {
                    let role = format!("block {index} lane mask");
                    for value in mask.expressions() {
                        let mut nested = Vec::new();
                        self.expr_nodes(&role, "lane mask", value, &mut nested);
                    }
                }
                if let MirInstr::ComptimeAssert { cond } = instruction {
                    self.constraint(&format!("block {index} compile-time assertion"), cond);
                }
                if let MirInstr::Const {
                    k: Const::Param(value),
                    ..
                } = instruction
                {
                    let role = format!("block {index} parameter constant");
                    let mut nested = Vec::new();
                    self.expr_nodes(&role, "parameter constant", value, &mut nested);
                }
            }
        }
    }

    /// A compile-time loop's binder is in scope, and every expression of its
    /// sequence is well-kinded.
    fn comptime_loop(
        &mut self,
        role: &str,
        binder: &mojito_types::param_expr::ParamRef,
        source: &mojito_checked::checked::ComptimeSequence,
    ) {
        self.reference(role, &binder.id, &binder.name, None, &[]);
        for expression in source.expressions() {
            // A sequence the elaborator evaluates is the application of its
            // thunk, a list of values; its arguments are held to every
            // expression's rules.
            let operands: Vec<&ParamExpr> = match expression.kind() {
                ParamKind::Apply { args, .. }
                    if matches!(expression.meta(), MetaTy::ParamList(element)
                        if matches!(**element, MetaTy::Value(_))) =>
                {
                    args.iter().collect()
                }
                _ => vec![expression],
            };
            for operand in operands {
                let mut nested = Vec::new();
                self.expr_nodes(role, "compile-time loop sequence", operand, &mut nested);
            }
        }
    }

    /// A compile-time branch condition: every binder it names is in scope,
    /// and every expression it holds is well-kinded.
    fn constraint(&mut self, role: &str, constraint: &GenericConstraint) {
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
        match constraint {
            Bool(_) => {}
            WithMessage(inner, _) | Not(inner) => self.constraint(role, inner),
            And(left, right) | Or(left, right) => {
                self.constraint(role, left);
                self.constraint(role, right);
            }
            PackPredicate { param, .. } => {
                self.reference(role, &param.id, &param.name, None, &[]);
            }
            PackContains { param, element } => {
                self.reference(role, &param.id, &param.name, None, &[]);
                self.operand(role, element);
            }
            Trivial(_, operand) | Conforms { operand, .. } | ConformsPack { operand, .. } => {
                self.operand(role, operand);
            }
            Eq(left, right)
            | Ne(left, right)
            | Lt(left, right)
            | Le(left, right)
            | Gt(left, right)
            | Ge(left, right) => {
                self.operand(role, left);
                self.operand(role, right);
            }
        }
    }

    fn operand(&mut self, role: &str, operand: &ConstraintOperand) {
        let mut nested = Vec::new();
        match operand {
            ConstraintOperand::Param(param) | ConstraintOperand::PackLength(param) => {
                self.reference(role, &param.id, &param.name, None, &nested);
            }
            ConstraintOperand::Value(CtValue::Expr(expr)) | ConstraintOperand::Expr(expr) => {
                self.expr_nodes(role, "compile-time condition", expr, &mut nested);
            }
            ConstraintOperand::Value(_) => {}
            ConstraintOperand::Type(ty) => self.walk_in(role, ty, &mut nested),
        }
    }

    /// Walk `ty` with the function's scope, entering each generic callable
    /// signature's own binders as it is passed.
    fn walk(&mut self, role: &str, ty: &Ty) {
        let mut nested = Vec::new();
        self.walk_in(role, ty, &mut nested);
    }

    fn walk_in(&mut self, role: &str, ty: &Ty, nested: &mut Vec<(ParamId, MetaTy)>) {
        match ty.kind() {
            TyKind::Param {
                binder,
                callable_bound,
                ..
            } => {
                self.reference(role, &binder.id, &binder.name, Some(&MetaTy::Type), nested);
                if let Some(callable) = callable_bound {
                    self.walk_in(role, callable, nested);
                }
            }
            TyKind::Assoc { base, args, .. } => {
                self.walk_in(role, base, nested);
                self.arguments(role, None, args, nested);
            }
            TyKind::Dependent(DependentType::Parameter(expr)) => {
                self.expr(role, "dependent type", expr, &MetaTy::Type, nested);
            }
            TyKind::Simd { dtype, width } => {
                if let SimdDtype::Expr(expr) = dtype {
                    self.expr(role, "lane", expr, &MetaTy::value(Ty::dtype()), nested);
                }
                if let SimdWidth::Expr(expr) = width {
                    self.expr(role, "width", expr, &MetaTy::int(), nested);
                }
            }
            TyKind::Struct(name, args) => {
                let declared = self
                    .declarations
                    .structure(name)
                    .map(|declaration| &declaration.param_decls[..]);
                self.erasable += 1;
                self.arguments(role, declared, args, nested);
                self.erasable -= 1;
            }
            TyKind::GenericFunc(generic) => {
                let GenericFuncTy {
                    decls,
                    func:
                        FuncTy {
                            params,
                            ret,
                            variadic,
                            kw_variadic,
                            error,
                            ..
                        },
                } = &**generic;

                let mark = nested.len();
                nested.extend(
                    decls
                        .iter()
                        .map(|decl| (decl.id().clone(), decl.binder_meta())),
                );
                for decl in decls {
                    match decl {
                        ParamDecl::Type {
                            callable_bound,
                            default,
                            ..
                        } => {
                            for ty in callable_bound.iter().chain(default.iter()) {
                                self.walk_in(role, ty, nested);
                            }
                        }
                        ParamDecl::Value { ty, .. } => self.walk_in(role, ty, nested),
                    }
                }
                for ty in params
                    .iter()
                    .chain(std::iter::once(ret))
                    .chain(variadic.iter())
                    .chain(kw_variadic.iter())
                    .chain(error.iter())
                {
                    self.walk_in(role, ty, nested);
                }
                nested.truncate(mark);
            }
            TyKind::Func(func) => {
                let FuncTy {
                    params,
                    ret,
                    variadic,
                    kw_variadic,
                    error,
                    ..
                } = &**func;

                for ty in params
                    .iter()
                    .chain(std::iter::once(ret))
                    .chain(variadic.iter())
                    .chain(kw_variadic.iter())
                    .chain(error.iter())
                {
                    self.walk_in(role, ty, nested);
                }
            }
            TyKind::Overload(types)
            | TyKind::Tuple(types)
            | TyKind::RuntimePack(types)
            | TyKind::Variant(types) => {
                for ty in types {
                    self.walk_in(role, ty, nested);
                }
            }
            TyKind::ComptimeList(element)
            | TyKind::ParamList(element)
            | TyKind::VariadicPack(element)
            | TyKind::PopArray { element, .. }
            | TyKind::Pointer { element, .. } => {
                self.walk_in(role, element, nested);
            }
            TyKind::Ref(reference) => self.walk_in(role, &reference.referent, nested),
            TyKind::Int
            | TyKind::UInt
            | TyKind::Bool
            | TyKind::StringLiteral
            | TyKind::Float64
            | TyKind::None
            | TyKind::Never
            | TyKind::IntLiteral
            | TyKind::FloatLiteral
            | TyKind::Infer
            | TyKind::Dtype
            | TyKind::AnyTrait(_)
            | TyKind::Trait(_)
            | TyKind::TypeType
            | TyKind::KgenString
            | TyKind::SelfType
            | TyKind::Error => {}
        }
    }

    /// A struct's or projection's arguments; a value argument at slot `i`
    /// has the kind the declared value parameter `i` gives it, a value
    /// pack's the list of its element.
    fn arguments(
        &mut self,
        role: &str,
        declared: Option<&[ParamDecl]>,
        args: &[TyArg],
        nested: &mut Vec<(ParamId, MetaTy)>,
    ) {
        // A spread of a list computed from packs still open is the whole
        // argument list, a `ParamList` rather than one type.
        if let Some(list) = mojito_types::types::list_spread_argument(args) {
            self.expr(role, "spread list", list, &MetaTy::type_list(), nested);
            return;
        }
        // A value parameter typed by a sibling type binder (`v: T`) has the
        // kind that binder's argument gives it.
        let types: mojito_types::types::TySubst = declared
            .into_iter()
            .flatten()
            .zip(args)
            .filter_map(|(decl, argument)| match (decl, argument) {
                (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => Some((id.clone(), ty.clone())),
                _ => None,
            })
            .collect();
        for (slot, argument) in args.iter().enumerate() {
            match argument {
                TyArg::Ty(ty) => self.walk_in(role, ty, nested),
                TyArg::Val(value) => {
                    let expected = declared
                        .and_then(|decls| decls.get(slot))
                        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
                        .map(ParamDecl::binder_meta)
                        .map(|expected| expected.substitute(&types));
                    self.value(role, value, expected.as_ref(), nested);
                }
                TyArg::Origin(_) => {}
            }
        }
    }

    fn value(
        &mut self,
        role: &str,
        value: &CtValue,
        expected: Option<&MetaTy>,
        nested: &mut Vec<(ParamId, MetaTy)>,
    ) {
        match value {
            CtValue::Expr(expr) => match expected {
                Some(expected) => self.expr(role, "value argument", expr, expected, nested),
                None => self.expr_nodes(role, "value argument", expr, nested),
            },
            CtValue::Type(ty) | CtValue::Reflected(ty) => self.walk_in(role, ty, nested),
            CtValue::Tuple(values)
            | CtValue::List(values)
            | CtValue::Set {
                elements: values, ..
            } => {
                for value in values {
                    self.value(role, value, None, nested);
                }
            }
            _ => {}
        }
    }

    /// An expression in a slot of kind `expected`: its own kind, then every
    /// reference below it.
    fn expr(
        &mut self,
        role: &str,
        slot: &str,
        expr: &ParamExpr,
        expected: &MetaTy,
        nested: &mut Vec<(ParamId, MetaTy)>,
    ) {
        if expr.meta() != expected && !expr.meta().is_trait_metatype_of(expected) {
            self.errors.push(format!(
                "{} {role} has {slot} `{expr}` of kind `{}`, not `{expected}`",
                self.head,
                expr.meta()
            ));
        }
        self.expr_nodes(role, slot, expr, nested);
    }

    fn expr_nodes(
        &mut self,
        role: &str,
        slot: &str,
        expr: &ParamExpr,
        nested: &mut Vec<(ParamId, MetaTy)>,
    ) {
        let mut embedded = Vec::new();
        let mut references = Vec::new();
        let mut findings = Vec::new();
        expr.visit(&mut |node| {
            embedded.extend(node.embedded_types().into_iter().cloned());
            match node.kind() {
                ParamKind::DeclRef(reference) => {
                    references.push((
                        reference.id.clone(),
                        reference.name.to_string(),
                        Some(node.meta().clone()),
                    ));
                }
                ParamKind::PackQuery { pack, .. } => {
                    references.push((pack.id.clone(), pack.name.to_string(), None));
                }
                ParamKind::Hole { .. } => {
                    findings.push(format!("{slot} `{expr}` holds an unbound hole"));
                }
                // A list of values is the sequence of a local display
                // binding, which an element or a length query reads.
                ParamKind::Apply { .. }
                    if !matches!(node.meta(), MetaTy::Value(_))
                        && !matches!(node.meta(), MetaTy::ParamList(element)
                            if matches!(**element, MetaTy::Value(_))) =>
                {
                    findings.push(format!(
                        "{slot} `{expr}` applies to a `{}` result, not a value",
                        node.meta()
                    ));
                }
                _ => {}
            }
        });
        for finding in findings {
            self.errors.push(format!("{} {role} {finding}", self.head));
        }
        let mut removed = self.removed_application_references(expr);
        for (id, name, meta) in references {
            let erasable = removed
                .get_mut(&id)
                .filter(|count| **count > 0)
                .map(|count| *count -= 1)
                .is_some();
            self.erasable += usize::from(erasable);
            self.reference(role, &id, &name, meta.as_ref(), nested);
            self.erasable -= usize::from(erasable);
        }
        for ty in embedded {
            self.walk_in(role, &ty, nested);
        }
    }

    /// How many times `expr` spells each binder inside an application's
    /// argument for a binder the lifted body it applies removed, by the
    /// body's snapshot of that body's removals.
    fn removed_application_references(&self, expr: &ParamExpr) -> HashMap<ParamId, usize> {
        let mut counts = HashMap::new();
        expr.visit(&mut |node| {
            let ParamKind::Apply { function, args } = node.kind() else {
                return;
            };
            let Some((removed, declaration)) = self
                .callee_removed_params
                .iter()
                .find(|(callee, _)| callee == function)
                .filter(|_| mojito_symbol::symbol::lifted_owner(function).is_some())
                .zip(self.declarations.function(function))
            else {
                return;
            };
            for (arg, decl) in args.iter().zip(&declaration.param_decls) {
                if removed.1.contains(decl.id()) {
                    arg.visit(&mut |inner| {
                        if let ParamKind::DeclRef(reference) = inner.kind() {
                            *counts.entry(reference.id.clone()).or_default() += 1;
                        }
                    });
                }
            }
        });
        counts
    }

    /// One reference to the binder `id`, spelled `name`, of kind `meta` when
    /// the reference records one.
    fn reference(
        &mut self,
        role: &str,
        id: &ParamId,
        name: &str,
        meta: Option<&MetaTy>,
        nested: &[(ParamId, MetaTy)],
    ) {
        if is_contract_binder(id) {
            return;
        }
        let declared = nested
            .iter()
            .rev()
            .find(|(nested, _)| nested == id)
            .map(|(_, kind)| kind)
            .or_else(|| self.scope.kinds.get(id))
            .or_else(|| {
                (self.erasable > 0)
                    .then(|| self.scope.erased.get(id))
                    .flatten()
            });
        match declared {
            None => self.errors.push(format!(
                "{} {role} names parameter `{name}` of `{}` that no enclosing declaration binds",
                self.head, id.owner
            )),
            Some(declared) => {
                // A pack spread is the pack's own `TyKind::Param`
                // (`types::pack_spread`), so a type reference to a variadic
                // binder is well-kinded.
                if let Some(meta) = meta
                    && meta != declared
                    && !meta.is_unresolved_struct(declared)
                    && !(*meta == MetaTy::Type && *declared == MetaTy::type_list())
                {
                    self.errors.push(format!(
                        "{} {role} names parameter `{name}` as `{meta}`, declared `{declared}`",
                        self.head
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::MirStructDeclaration;
    use mojito_types::param_expr::{ParamContext, ParamRef};

    fn binder(owner: &str, slot: usize, name: &str) -> ParamRef {
        ParamRef {
            id: ParamId::new(owner, slot),
            name: name.into(),
        }
    }

    fn function_with(reg_types: Vec<Ty>) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::TryYield(Vec::new()),
            }],
            n_regs: reg_types.len() as u32,
            n_vars: 0,
            var_names: Vec::new(),
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: Some(Ty::none()),
            raises: false,
            error_ty: None,
            error_slot: None,
            spans: crate::mir::SpanTable(HashMap::new()),
            reg_types: reg_types
                .into_iter()
                .enumerate()
                .map(|(index, ty)| (index as u32, ty))
                .collect(),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            callee_removed_args: Vec::new(),
            removed_args: Vec::new(),
            lowered_calls: Vec::new(),
        }
    }

    fn declarations(name: &str, decls: Vec<ParamDecl>) -> MirDeclarations {
        MirDeclarations {
            structs: Vec::new(),
            functions: vec![MirFunctionDeclaration {
                lowered_name: name.to_string(),
                param_names: Vec::new(),
                param_types: Vec::new(),
                defaults: Vec::new(),
                required: Vec::new(),
                variadic: None,
                variadic_convention: None,
                variadic_index: None,
                kw_variadic: None,
                kw_variadic_convention: None,
                kw_variadic_index: None,
                positional_only: None,
                keyword_only: None,
                param_decls: decls,
                has_receiver: false,
                receiver_convention: None,
                param_conventions: Vec::new(),
                ret_ty: Ty::none(),
                returns_reference: false,
                raises: false,
                error_ty: None,
                ref_params: Vec::new(),
                param_writes: Vec::new(),
                availability: Vec::new(),
                enclosing: None,
                region_binders: Vec::new(),
                inline: mojito_checked::declaration::InlineLevel::default(),
                provenance: mojito_checked::declaration::FunctionProvenance::default(),
            }],
            traits: Vec::new(),
            imported: None,
        }
    }

    fn value_decl(owner: &str, slot: usize, name: &str, ty: Ty) -> ParamDecl {
        ParamDecl::Value {
            id: ParamId::new(owner, slot),
            name: name.to_string(),
            ty,
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }
    }

    fn findings(function: &MirFunction, declarations: &MirDeclarations) -> Vec<String> {
        let mut errors = Vec::new();
        verify_scope("f", function, declarations, &mut errors);
        errors
    }

    /// A vector over the body's own value binders is well-scoped and
    /// well-kinded in parametric MIR, and still rejected concrete.
    #[test]
    fn scope_accepts_a_symbolic_vector_over_declared_binders() {
        let context = ParamContext::detached();
        let dt = context.decl_ref(ParamId::new("f", 0), "dt", MetaTy::value(Ty::dtype()));
        let width = context.decl_ref(ParamId::new("f", 1), "width", MetaTy::int());
        let symbolic = TyKind::Simd {
            dtype: SimdDtype::Expr(dt),
            width: SimdWidth::Expr(width),
        };
        let function = function_with(vec![symbolic.into()]);
        let declarations = declarations(
            "f",
            vec![
                value_decl("f", 0, "dt", Ty::dtype()),
                value_decl("f", 1, "width", Ty::int()),
            ],
        );
        assert_eq!(findings(&function, &declarations), Vec::<String>::new());
        let concrete = super::super::concrete_function_findings("f", &function);
        assert!(
            concrete
                .iter()
                .any(|finding| finding.contains("keeps symbolic type")),
            "{concrete:?}"
        );
    }

    /// A binder of another declaration, a width of the wrong kind, and a
    /// reference whose kind is not the declared one are each a finding.
    #[test]
    fn scope_rejects_unbound_and_mistyped_binders() {
        let context = ParamContext::detached();
        let foreign = context.decl_ref(ParamId::new("g", 0), "n", MetaTy::int());
        let flag = context.decl_ref(ParamId::new("f", 0), "flag", MetaTy::bool());
        let declarations = declarations("f", vec![value_decl("f", 0, "flag", Ty::bool())]);
        let unbound = function_with(vec![
            TyKind::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(foreign),
            }
            .into(),
        ]);
        assert!(
            findings(&unbound, &declarations)
                .iter()
                .any(|finding| finding.contains("no enclosing declaration binds")),
        );
        let mistyped = function_with(vec![
            TyKind::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(flag),
            }
            .into(),
        ]);
        assert!(
            findings(&mistyped, &declarations)
                .iter()
                .any(|finding| finding.contains("of kind `Bool`, not `Int`")),
        );
        let misdeclared = function_with(vec![
            TyKind::Param {
                binder: binder("f", 0, "flag"),
                bounds: Vec::new(),
                callable_bound: None,
            }
            .into(),
        ]);
        assert!(
            findings(&misdeclared, &declarations)
                .iter()
                .any(|finding| finding.contains("no enclosing declaration binds")
                    || finding.contains("declared `Bool`")),
        );
    }

    /// A method call's solved argument is scope-checked like any type the
    /// body spells: one naming a binder no enclosing declaration declares is
    /// a finding.
    #[test]
    fn scope_rejects_a_method_argument_over_an_unbound_binder() {
        let declarations = declarations("f", Vec::new());
        let mut function = function_with(vec![Ty::none(), Ty::none()]);
        function.blocks[0].instrs.push(MirInstr::MethodCall {
            dest: Reg(1),
            recv: Reg(0),
            method: "show".into(),
            resolved: Some("S.show".into()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_place: None,
            recv_writes: false,
            receiver_convention: None,
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: vec![TyArg::Ty(
                TyKind::Param {
                    binder: binder("g", 0, "T"),
                    bounds: Vec::new(),
                    callable_bound: None,
                }
                .into(),
            )],
            spread: None,
        });
        assert!(
            findings(&function, &declarations)
                .iter()
                .any(|finding| finding.contains("instantiated argument")
                    && finding.contains("no enclosing declaration binds")),
            "{:?}",
            findings(&function, &declarations)
        );
    }

    /// A parameter the generator removed stays in scope inside a struct
    /// argument whose layout does not read it, which an instance erases
    /// (`_REMOVED_ARG`), and nowhere else.
    #[test]
    fn scope_keeps_a_removed_parameter_in_struct_arguments_only() {
        let context = ParamContext::detached();
        let n = context.decl_ref(ParamId::new("f", 0), "n", MetaTy::int());
        let mut declarations = declarations("f", vec![value_decl("f", 0, "n", Ty::int())]);
        declarations.structs.push(MirStructDeclaration {
            name: "Box".into(),
            fields: vec![("v".into(), Ty::int())],
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: vec![value_decl("Box", 0, "n", Ty::int())],
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: Vec::new(),
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            parameterized_associated_values: Vec::new(),
            witness_tables: Vec::new(),
            members: Vec::new(),
        });
        let boxed = TyKind::Struct("Box".into(), vec![TyArg::Val(CtValue::Expr(n.clone()))]);
        let mut erased = function_with(vec![boxed.into()]);
        erased.removed_params = vec![ParamId::new("f", 0)];
        assert_eq!(findings(&erased, &declarations), Vec::<String>::new());
        let mut read = function_with(vec![
            TyKind::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(n),
            }
            .into(),
        ]);
        read.removed_params = vec![ParamId::new("f", 0)];
        let found = findings(&read, &declarations);
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("no enclosing declaration binds")),
            "{found:?}"
        );
    }

    /// A parameter the generator removed stays in scope as an application's
    /// argument for a binder the lifted body it applies removed too, by the
    /// body's snapshot of that body's removals, and nowhere else.
    #[test]
    fn scope_keeps_a_removed_parameter_in_a_removed_application_argument() {
        let context = ParamContext::detached();
        let n = context.decl_ref(ParamId::new("f", 0), "n", MetaTy::int());
        let lifted = "$comptime$f$0";
        let mut declarations = declarations("f", vec![value_decl("f", 0, "n", Ty::int())]);
        let mut thunk = declarations.functions[0].clone();
        thunk.lowered_name = lifted.into();
        declarations.functions.push(thunk);
        let width = |argument: &ParamExpr| {
            TyKind::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(context.apply(
                    lifted,
                    std::slice::from_ref(argument),
                    MetaTy::int(),
                )),
            }
            .into()
        };
        let mut erased = function_with(vec![width(&n)]);
        erased.removed_params = vec![ParamId::new("f", 0)];
        erased.callee_removed_params = vec![(lifted.into(), vec![ParamId::new("f", 0)])];
        assert_eq!(findings(&erased, &declarations), Vec::<String>::new());
        let mut kept = erased;
        kept.callee_removed_params = vec![(lifted.into(), Vec::new())];
        let found = findings(&kept, &declarations);
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("no enclosing declaration binds")),
            "{found:?}"
        );
    }

    /// A removed argument is used by nothing but a drop that runs no code:
    /// its drop is no use, and any other instruction naming it is.
    #[test]
    fn scope_rejects_a_removed_argument_the_body_uses() {
        let mut declarations = declarations("f", Vec::new());
        let declaration = &mut declarations.functions[0];
        declaration.param_names = vec!["a".to_string()];
        declaration.param_types = vec![Ty::int()];
        declaration.defaults = vec![None];
        declaration.required = vec![true];
        declaration.param_conventions = vec![None];
        declaration.ref_params = vec![false];
        declaration.param_writes = vec![false];
        let with = |instrs: Vec<MirInstr>| MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(None),
            }],
            n_regs: 1,
            n_vars: 1,
            var_names: vec!["a".to_string()],
            n_params: 1,
            param_types: vec![Ty::int()],
            owned_params: vec![true],
            deinit_params: vec![false],
            ref_params: vec![false],
            var_tys: HashMap::from([(0, Ty::int())]),
            removed_args: vec![0],
            lowered_calls: Vec::new(),
            ..function_with(vec![Ty::int()])
        };
        let dropped = with(vec![MirInstr::DropVar {
            var: 0,
            destructor: None,
        }]);
        assert_eq!(findings(&dropped, &declarations), Vec::<String>::new());
        let read = with(vec![MirInstr::UseVar {
            dest: Reg(0),
            var: 0,
            mode: crate::mir::UseMode::Copy,
            constructor: None,
        }]);
        assert_eq!(
            findings(&read, &declarations),
            vec!["MIR function 'f' removes argument slot 0, which it uses".to_string()]
        );
    }

    /// A removed argument the body only forwards to a callee's removed
    /// argument is unused, but only for a callee its snapshot names.
    #[test]
    fn scope_reads_a_forward_to_a_removed_argument_through_the_snapshot() {
        let mut declarations = declarations("f", Vec::new());
        let declaration = &mut declarations.functions[0];
        declaration.param_names = vec!["a".to_string()];
        declaration.param_types = vec![Ty::int()];
        declaration.defaults = vec![None];
        declaration.required = vec![true];
        declaration.param_conventions = vec![None];
        declaration.ref_params = vec![false];
        declaration.param_writes = vec![false];
        let mut callee = declarations.functions[0].clone();
        callee.lowered_name = "g".to_string();
        declarations.functions.push(callee);
        let forward = |snapshot: Vec<(String, Vec<mojito_hir::hir::VarId>)>| MirFunction {
            blocks: vec![MirBlock {
                instrs: vec![
                    MirInstr::UseVar {
                        dest: Reg(0),
                        var: 0,
                        mode: crate::mir::UseMode::Copy,
                        constructor: None,
                    },
                    MirInstr::Call {
                        dest: Reg(1),
                        func: crate::mir::FuncRef::named("g"),
                        raises: None,
                        args: vec![Reg(0)],
                        kwargs: Vec::new(),
                        arg_places: vec![None],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        receiver: None,
                        instantiated_args: Vec::new(),
                        spread: None,
                    },
                ],
                term: MirTerm::Return(None),
            }],
            n_regs: 2,
            n_vars: 1,
            var_names: vec!["a".to_string()],
            n_params: 1,
            param_types: vec![Ty::int()],
            owned_params: vec![false],
            deinit_params: vec![false],
            ref_params: vec![false],
            var_tys: HashMap::from([(0, Ty::int())]),
            removed_args: vec![0],
            lowered_calls: Vec::new(),
            callee_removed_args: snapshot,
            ..function_with(vec![Ty::int(), Ty::int()])
        };
        assert_eq!(
            findings(&forward(vec![("g".to_string(), vec![0])]), &declarations),
            Vec::<String>::new()
        );
        assert_eq!(
            findings(&forward(Vec::new()), &declarations),
            vec!["MIR function 'f' removes argument slot 0, which it uses".to_string()]
        );
    }

    /// A pack element over a declared variadic binder and its loop index is
    /// in scope, and a contract binder is bound by its contract.
    #[test]
    fn scope_accepts_pack_elements_and_contract_binders() {
        let context = ParamContext::detached();
        let pack = context.decl_ref(ParamId::new("Bag", 0), "Ts", MetaTy::type_list());
        let index = context.decl_ref(ParamId::new("Bag.first", 0), "i", MetaTy::int());
        let element = context.list_get(&pack, &index).expect("element builds");
        let callable = context.decl_ref(ParamId::new("$callable", 0), "index", MetaTy::int());
        let function = function_with(vec![
            TyKind::Dependent(DependentType::Parameter(element)).into(),
            TyKind::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(callable),
            }
            .into(),
        ]);
        let mut declarations = declarations(
            "Bag.first",
            vec![value_decl("Bag.first", 0, "i", Ty::int())],
        );
        declarations.structs.push(MirStructDeclaration {
            name: "Bag".into(),
            fields: Vec::new(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: vec![ParamDecl::Type {
                id: ParamId::new("Bag", 0),
                name: "Ts".into(),
                bounds: Vec::new(),
                trait_param: None,
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: true,
                constraints: Vec::new(),
            }],
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: Vec::new(),
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            parameterized_associated_values: Vec::new(),
            witness_tables: Vec::new(),
            members: Vec::new(),
        });
        let mut errors = Vec::new();
        verify_scope("Bag.first", &function, &declarations, &mut errors);
        assert_eq!(errors, Vec::<String>::new());
    }
}
