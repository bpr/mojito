//! The binder scope and kind of every parametric type a function names.
//!
//! Parametric MIR names a type parameter (`Ty::Param`), a dependent type, a
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
use mojito_types::types::{
    ConstraintOperand, DependentType, GenericConstraint, SimdDtype, SimdWidth,
};

/// The scope and kind findings of one function body.
pub(super) fn verify_scope(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    let mut scope = Scope::of_function(name, declarations);
    scope.declare_loop_indices(&function.blocks);
    let head = format!("MIR function '{name}'");
    let mut cx = ScopeCx {
        scope: &scope,
        declarations,
        head: &head,
        errors,
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
        | MirInstr::TryNext { exhaustion: ty, .. }
        | MirInstr::DefVar {
            binding_ty: Some(ty),
            ..
        } => types.push(("instruction", ty)),
        MirInstr::TypeName { ty, .. } => types.push(("type name", ty)),
        MirInstr::Call {
            raises,
            receiver,
            instantiated_args,
            ..
        } => {
            types.extend(
                raises
                    .iter()
                    .chain(receiver.iter())
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
                    .iter()
                    .chain(instantiated_contract.iter())
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
            types.extend(raises.iter().map(|ty| ("error contract", ty)));
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
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => {
            types.extend(
                call.raises
                    .iter()
                    .chain(std::iter::once(&call.result_ty))
                    .map(|ty| ("subscript contract", ty)),
            );
        }
        _ => {}
    }
    types
}

/// Whether a binder is one a callable contract or witness request spells,
/// bound by the construct that holds it rather than by a declaration.
pub(super) fn is_contract_binder(id: &ParamId) -> bool {
    id.owner.starts_with('$')
}

/// The binders a body may name, with the kind each declares.
struct Scope {
    kinds: HashMap<ParamId, MetaTy>,
}

impl Scope {
    fn of_function(name: &str, declarations: &MirDeclarations) -> Self {
        let own = declarations
            .functions
            .iter()
            .find(|declaration| declaration.lowered_name == name)
            .map_or(&[][..], |declaration| &declaration.param_decls);
        let owner = name.split_once('.').and_then(|(owner, _)| {
            declarations
                .structs
                .iter()
                .find(|declaration| declaration.name == owner)
                .map(|declaration| &declaration.param_decls[..])
        });
        let mut scope = Self {
            kinds: HashMap::new(),
        };
        scope.declare(own);
        scope.declare(owner.unwrap_or(&[]));
        scope
    }

    /// Bring every `comptime for` index of `blocks` into scope: a loop's own
    /// `Int` binder, declared by no signature, that the body's types and
    /// conditions name.
    fn declare_loop_indices(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            if let MirTerm::ComptimeFor { index, .. } = &block.term {
                self.kinds.insert(index.id.clone(), MetaTy::value(Ty::Int));
            }
            for instruction in &block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    for region in std::iter::once(body)
                        .chain(handler.iter().map(|(_, blocks)| blocks))
                        .chain(orelse.iter())
                        .chain(finalbody.iter())
                    {
                        self.declare_loop_indices(region);
                    }
                }
            }
        }
    }

    fn declare(&mut self, decls: &[ParamDecl]) {
        for decl in decls {
            self.kinds.insert(decl.id().clone(), declared_kind(decl));
        }
    }
}

/// The kind a declaration's binder is referenced at.
pub fn declared_kind(decl: &ParamDecl) -> MetaTy {
    match decl {
        ParamDecl::Value { ty, .. } => MetaTy::value((**ty).clone()),
        ParamDecl::Type { variadic: true, .. } => MetaTy::type_list(),
        ParamDecl::Type { .. } => MetaTy::Type,
    }
}

struct ScopeCx<'a> {
    scope: &'a Scope,
    declarations: &'a MirDeclarations,
    head: &'a str,
    errors: &'a mut Vec<String>,
}

impl ScopeCx<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for (index, block) in blocks.iter().enumerate() {
            for instruction in &block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    let regions = std::iter::once(body)
                        .chain(handler.iter().map(|(_, blocks)| blocks))
                        .chain(orelse.iter())
                        .chain(finalbody.iter());
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
                    let built = Ty::Simd {
                        dtype: dtype.clone(),
                        width: width.clone(),
                    };
                    self.walk(&format!("block {index} vector slots"), &built);
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
            match &block.term {
                MirTerm::ComptimeBranch { cond, .. } => {
                    self.constraint(&format!("block {index} compile-time branch"), cond);
                }
                MirTerm::ComptimeFor {
                    index: binder,
                    start,
                    stop,
                    step,
                    ..
                } => {
                    let role = format!("block {index} compile-time loop");
                    self.reference(&role, &binder.id, &binder.name, None, &[]);
                    for bound in [start, stop, step] {
                        let mut nested = Vec::new();
                        self.expr_nodes(&role, "compile-time loop bound", bound, &mut nested);
                    }
                }
                _ => {}
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
            Conforms { param, .. } | ConformsPack { param, .. } | PackPredicate { param, .. } => {
                self.reference(role, &param.id, &param.name, None, &[]);
            }
            PackContains { param, element } => {
                self.reference(role, &param.id, &param.name, None, &[]);
                self.operand(role, element);
            }
            Trivial(_, operand) => self.operand(role, operand),
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
        match ty {
            Ty::Param {
                binder,
                callable_bound,
                ..
            } => {
                self.reference(role, &binder.id, &binder.name, Some(&MetaTy::Type), nested);
                if let Some(callable) = callable_bound {
                    self.walk_in(role, callable, nested);
                }
            }
            Ty::Assoc { base, args, .. } => {
                self.walk_in(role, base, nested);
                self.arguments(role, None, args, nested);
            }
            Ty::Dependent(DependentType::Parameter(expr)) => {
                self.expr(role, "dependent type", expr, &MetaTy::Type, nested);
            }
            Ty::Simd { dtype, width } => {
                if let SimdDtype::Expr(expr) = dtype {
                    self.expr(role, "lane", expr, &MetaTy::value(Ty::Dtype), nested);
                }
                if let SimdWidth::Expr(expr) = width {
                    self.expr(role, "width", expr, &MetaTy::int(), nested);
                }
            }
            Ty::Struct(name, args) => {
                let declared = self
                    .declarations
                    .structs
                    .iter()
                    .find(|declaration| &declaration.name == name)
                    .map(|declaration| &declaration.param_decls[..]);
                self.arguments(role, declared, args, nested);
            }
            Ty::GenericFunc {
                decls,
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                let mark = nested.len();
                nested.extend(
                    decls
                        .iter()
                        .map(|decl| (decl.id().clone(), declared_kind(decl))),
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
                    .chain(std::iter::once(&**ret))
                    .chain(variadic.iter().map(|ty| &**ty))
                    .chain(kw_variadic.iter().map(|ty| &**ty))
                    .chain(error.iter().map(|ty| &**ty))
                {
                    self.walk_in(role, ty, nested);
                }
                nested.truncate(mark);
            }
            Ty::Func {
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                for ty in params
                    .iter()
                    .chain(std::iter::once(&**ret))
                    .chain(variadic.iter().map(|ty| &**ty))
                    .chain(kw_variadic.iter().map(|ty| &**ty))
                    .chain(error.iter().map(|ty| &**ty))
                {
                    self.walk_in(role, ty, nested);
                }
            }
            Ty::Overload(types)
            | Ty::Tuple(types)
            | Ty::RuntimePack(types)
            | Ty::Variant(types) => {
                for ty in types {
                    self.walk_in(role, ty, nested);
                }
            }
            Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
                self.walk_in(role, element, nested);
            }
            Ty::Ref(reference) => self.walk_in(role, &reference.referent, nested),
            Ty::Int
            | Ty::UInt
            | Ty::Bool
            | Ty::StringLiteral
            | Ty::Float64
            | Ty::None
            | Ty::Never
            | Ty::IntLiteral
            | Ty::FloatLiteral
            | Ty::Infer
            | Ty::Dtype
            | Ty::SelfType
            | Ty::Error => {}
        }
    }

    /// A struct's or projection's arguments; a value argument at slot `i`
    /// has the kind the declared value parameter `i` gives it.
    fn arguments(
        &mut self,
        role: &str,
        declared: Option<&[ParamDecl]>,
        args: &[TyArg],
        nested: &mut Vec<(ParamId, MetaTy)>,
    ) {
        for (slot, argument) in args.iter().enumerate() {
            match argument {
                TyArg::Ty(ty) => self.walk_in(role, ty, nested),
                TyArg::Val(value) => {
                    let expected =
                        declared
                            .and_then(|decls| decls.get(slot))
                            .and_then(|decl| match decl {
                                ParamDecl::Value { ty, .. } => Some(MetaTy::value((**ty).clone())),
                                ParamDecl::Type { .. } => None,
                            });
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
        if expr.meta() != expected {
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
                ParamKind::Apply { .. } if !matches!(node.meta(), MetaTy::Value(_)) => {
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
        for (id, name, meta) in references {
            self.reference(role, &id, &name, meta.as_ref(), nested);
        }
        for ty in embedded {
            self.walk_in(role, &ty, nested);
        }
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
            .or_else(|| self.scope.kinds.get(id));
        match declared {
            None => self.errors.push(format!(
                "{} {role} names parameter `{name}` of `{}` that no enclosing declaration binds",
                self.head, id.owner
            )),
            Some(declared) => {
                // A pack spread is the pack's own `Ty::Param`
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
                term: MirTerm::FallOff,
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
            ret_ty: Some(Ty::None),
            raises: false,
            error_ty: None,
            spans: crate::mir::SpanTable(HashMap::new()),
            reg_types: reg_types
                .into_iter()
                .enumerate()
                .map(|(index, ty)| (index as u32, ty))
                .collect(),
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
                ret_ty: Ty::None,
                returns_reference: false,
                raises: false,
                error_ty: None,
                ref_params: Vec::new(),
                param_writes: Vec::new(),
                availability: Vec::new(),
            }],
            traits: Vec::new(),
        }
    }

    fn value_decl(owner: &str, slot: usize, name: &str, ty: Ty) -> ParamDecl {
        ParamDecl::Value {
            id: ParamId::new(owner, slot),
            name: name.to_string(),
            ty: Box::new(ty),
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
        let dt = context.decl_ref(ParamId::new("f", 0), "dt", MetaTy::value(Ty::Dtype));
        let width = context.decl_ref(ParamId::new("f", 1), "width", MetaTy::int());
        let symbolic = Ty::Simd {
            dtype: SimdDtype::Expr(dt),
            width: SimdWidth::Expr(width),
        };
        let function = function_with(vec![symbolic]);
        let declarations = declarations(
            "f",
            vec![
                value_decl("f", 0, "dt", Ty::Dtype),
                value_decl("f", 1, "width", Ty::Int),
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
        let declarations = declarations("f", vec![value_decl("f", 0, "flag", Ty::Bool)]);
        let unbound = function_with(vec![Ty::Simd {
            dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
            width: SimdWidth::Expr(foreign),
        }]);
        assert!(
            findings(&unbound, &declarations)
                .iter()
                .any(|finding| finding.contains("no enclosing declaration binds")),
        );
        let mistyped = function_with(vec![Ty::Simd {
            dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
            width: SimdWidth::Expr(flag),
        }]);
        assert!(
            findings(&mistyped, &declarations)
                .iter()
                .any(|finding| finding.contains("of kind `Bool`, not `Int`")),
        );
        let misdeclared = function_with(vec![Ty::Param {
            binder: binder("f", 0, "flag"),
            bounds: Vec::new(),
            callable_bound: None,
        }]);
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
        let mut function = function_with(vec![Ty::None, Ty::None]);
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
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: vec![TyArg::Ty(Ty::Param {
                binder: binder("g", 0, "T"),
                bounds: Vec::new(),
                callable_bound: None,
            })],
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
            Ty::Dependent(DependentType::Parameter(element)),
            Ty::Simd {
                dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                width: SimdWidth::Expr(callable),
            },
        ]);
        let mut declarations =
            declarations("Bag.first", vec![value_decl("Bag.first", 0, "i", Ty::Int)]);
        declarations.structs.push(MirStructDeclaration {
            name: "Bag".into(),
            fields: Vec::new(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: vec![ParamDecl::Type {
                id: ParamId::new("Bag", 0),
                name: "Ts".into(),
                bounds: Vec::new(),
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: true,
                constraints: Vec::new(),
            }],
            explicit_destroy_message: None,
            explicit_destructors: HashMap::new(),
            conformances: Vec::new(),
            associated_types: Vec::new(),
        });
        let mut errors = Vec::new();
        verify_scope("Bag.first", &function, &declarations, &mut errors);
        assert_eq!(errors, Vec::<String>::new());
    }
}
