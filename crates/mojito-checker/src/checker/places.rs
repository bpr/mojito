//! Place construction, overlap, mutability, and origin derivation for checking.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::TransferVerdict;
use mojito_types::types::TyKind;

#[allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take Option<&T>; TODO: take by value and update the call sites"
)]
pub(super) const fn parameter_is_writable(convention: &Option<ArgConvention>) -> bool {
    matches!(
        convention,
        Some(ArgConvention::Mut | ArgConvention::Out | ArgConvention::Var | ArgConvention::Deinit)
    )
}

/// Solve a type parameter against an actual type, recording it in `subst`. A
/// numeric literal is defaulted to its concrete type first (`IntLiteral → Int`,
/// `FloatLiteral → Float64`) so the solution matches the value the VM stores —
/// this deliberately forbids widening one literal to match
/// another across arguments (e.g. `Pair(1.0, 2)` is a conflict, not `Pair[Float64]`).
/// Whether an expression is a **place** — it names an existing binding (a variable
/// or a field/index chain rooted at one) rather than producing a fresh value. A
/// `^` transfer, a call result, a literal, or an operator is *not* a place.
impl Checker {
    /// Record how a selected call's read parameters bind their arguments:
    /// the place arguments it may borrow instead of copying
    /// ([`borrowable_read_arguments`]) and the owned temporaries it borrows,
    /// which the caller destroys once the call returns
    /// ([`read_temporary_arguments`]). The arguments a read `*args`
    /// collector gathers bind the same way, and so does a temporary
    /// receiver whose declared convention is a read
    /// ([`read_temporary_receiver`]); such a place receiver is marked so a
    /// register-passable one is passed by value ([`read_place_receiver`]),
    /// as is a register-passable `deinit` receiver.
    pub(super) fn record_argument_borrows(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        gathered: Gathered<'_>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        receiver: Option<(&Expr, Option<ArgConvention>)>,
    ) {
        // A converted place's argument is the conversion's result, a
        // temporary of its own.
        let designates_place = |arg: &Expr| {
            self.designates_place(arg)
                && !self.has_fact(&arg.source_span(), |facts| {
                    facts.implicit_conversion.is_some()
                })
        };
        let read_place = |arg: &Expr| {
            if self.infer_reference_value(arg).is_some() {
                ReadPlace::ReferenceResult
            } else if self.designates_place(arg) {
                ReadPlace::Place
            } else {
                ReadPlace::Value
            }
        };
        let collected = gathered.read();
        // A reference result at a `mut` or `ref` parameter binds its
        // referent, as upstream binds the returned reference to the
        // parameter: it is never read out as a value.
        for span in exclusive_reference_arguments(slots, conventions, args, kwargs)
            .into_iter()
            .filter(|arg| self.infer_reference_value(arg).is_some())
            .map(Expr::source_span)
        {
            self.borrowed_reference_receivers.borrow_mut().insert(span);
        }
        for span in borrowable_read_arguments(
            slots,
            conventions,
            collected,
            args,
            kwargs,
            receiver,
            read_place,
        ) {
            self.record_at(span).borrowed_read_call_place = true;
        }
        let mut read = read_temporary_arguments(
            slots,
            conventions,
            collected,
            args,
            kwargs,
            designates_place,
        );
        read.extend(read_temporary_receiver(receiver, designates_place));
        self.unconsumed_temporaries
            .borrow_mut()
            .extend(read.iter().cloned());
        for span in read {
            self.record_at(span).read_temporary_argument = true;
        }
        if let Some(span) = read_place_receiver(receiver, designates_place) {
            self.record_at(span).read_place_receiver = true;
        }
        // A `^` receiver is lowered as its operand, which takes the verdict
        // too.
        if let Some((receiver, _)) =
            receiver.filter(|(_, convention)| *convention == Some(ArgConvention::Deinit))
        {
            self.record_at(receiver.source_span()).consumed_receiver = true;
            if let ExprKind::Transfer(operand) = &receiver.kind {
                self.record_at(operand.source_span()).consumed_receiver = true;
            }
        }
    }

    /// Mark the place receiver of a contract-less builtin method upstream
    /// declares with a `read self`, so a register-passable one is passed by
    /// value ([`read_place_receiver`]), as for a declared `read` receiver.
    pub(super) fn record_builtin_read_receiver(&self, receiver: &Expr) {
        if let Some(span) = read_place_receiver(Some((receiver, Some(ArgConvention::Imm))), |arg| {
            self.designates_place(arg)
        }) {
            self.record_at(span).read_place_receiver = true;
        }
    }

    /// Record the conventions `site`, a call that records no contract,
    /// binds its arguments by: each bound slot's effective convention, and
    /// a gathering collector's for each element it gathers
    /// ([`SemanticAdjustment::CallConventions`]), with what each bound
    /// slot's declaration says of it (`declared`).
    pub(super) fn record_call_conventions(
        &self,
        site: &SourceSpan,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        declared: &DeclaredSlots,
        gathered: Gathered<'_>,
    ) {
        self.record_at(site.clone()).call_conventions =
            Some(argument_conventions(slots, conventions, declared, gathered));
    }

    /// What a signature's declaration says of each of its parameters, by
    /// its declared `conventions`, `ref` contracts (`ref_params`), and
    /// parameter types (`declared`, else `bound`).
    pub(super) fn declared_slots(
        &self,
        conventions: &[Option<ArgConvention>],
        ref_params: &[Option<mojito_types::origin::RefSig>],
        declared: &[Ty],
        bound: &[Ty],
    ) -> DeclaredSlots {
        DeclaredSlots {
            by_address: self.declared_by_address_slots(conventions, declared, bound),
            writes: mojito_checked::checked::DeclarationEffect::param_writes(
                conventions,
                ref_params,
            ),
        }
    }

    /// Bind each nominal subscript passed to a `mut` parameter of `func` as
    /// a mutable place: a value getter with a setter is written back around
    /// the call ([`Self::check_place`]), as upstream passes a
    /// `SubscriptDLValue` through a temporary; a getter without a setter
    /// must return a mutable reference.
    pub(super) fn bind_mutable_subscript_arguments(
        &self,
        func: &str,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        for (index, slot) in slots.iter().enumerate() {
            if !matches!(conventions.get(index), Some(Some(ArgConvention::Mut))) {
                continue;
            }
            let argument = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            match self.subscript_lvalue(argument)? {
                Some(super::indexing::SubscriptLValue::GetterSetter {
                    getter_returns_ref: false,
                    ..
                }) => {
                    self.check_place(argument)?;
                }
                Some(super::indexing::SubscriptLValue::Getter)
                    if self.is_owned_subscript(argument) =>
                {
                    return Err(TypeError::BadCall {
                        func: func.to_string(),
                        reason: "value passed to a mutable argument must be mutable; a \
                                 subscript whose '__getitem__' returns a value and that \
                                 declares no '__setitem__' is not a place"
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Whether `expr` designates a place rather than a fresh value: a
    /// syntactic place ([`is_place_expr`]) other than a subscript whose
    /// selected getter returns by value. A subscript is its getter's call,
    /// so `b[i]` over `def __getitem__(self, i: Int) -> T` is an owned
    /// temporary, as upstream; a `ref` getter, a pointer subscript, and a
    /// storage read stay places. A name or member reading a recorded
    /// parameter value is a fresh value too, as upstream's parameter
    /// reference is an rvalue.
    pub(super) fn designates_place(&self, expr: &Expr) -> bool {
        is_place_expr(expr)
            && !self.is_owned_subscript(expr)
            && !self.is_param_value_read(expr)
            && !self.walrus_stores_through_setter(expr)
    }

    /// The destination the walrus `e` yields borrowed after its store
    /// ([`walrus_destination`]), unless its target stores through a
    /// subscript's `__setitem__`: upstream's `SubscriptDLValue` is no memory
    /// lvalue, so `emitWalrus` yields that walrus's right-hand side.
    pub(super) fn walrus_place<'e>(&self, e: &'e Expr) -> Option<&'e Expr> {
        walrus_destination(e).filter(|_| !self.walrus_stores_through_setter(e))
    }

    /// Whether `e` is a walrus whose target stores through a subscript's
    /// `__setitem__` (upstream's `SubscriptDLValue`).
    pub(super) fn walrus_stores_through_setter(&self, e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Named { target, .. }
        if self.has_fact(&target.source_span(), |facts| {
            facts
                .subscript_descriptor
                .as_ref()
                .is_some_and(|(_, set_value_keyword)| set_value_keyword.is_some())
        }))
    }

    /// The positions `name`'s positional collector gathers and whether it
    /// owns them (a `var` collector). Overloads that disagree leave the
    /// convention unknown, which gathers none.
    pub(super) fn gathered_arguments<'a>(&self, name: &str, overflow: &'a [usize]) -> Gathered<'a> {
        self.owned_collectors
            .get(name)
            .map_or(Gathered::NONE, |owning| Gathered {
                positions: overflow,
                owning: *owning,
            })
    }

    /// Record an expression whose value nothing takes ownership of: a
    /// temporary a callee only borrows, or a discarded statement expression.
    /// A linear one is abandoned there ([`Self::record_linear_temporary`]).
    pub(super) fn record_unconsumed_temporary(&self, expr: &Expr) {
        if !self.designates_place(expr) && !matches!(expr.kind, ExprKind::Transfer(_)) {
            self.unconsumed_temporaries
                .borrow_mut()
                .insert(expr.source_span());
        }
    }

    /// Record an owned temporary a builtin reads, as a `read` parameter
    /// binds one ([`read_temporary_arguments`]): the caller destroys it once
    /// the call returns.
    pub(super) fn record_read_temporary_argument(&self, expr: &Expr) {
        if !self.designates_place(expr) && !matches!(expr.kind, ExprKind::Transfer(_)) {
            self.record_at(expr.source_span()).read_temporary_argument = true;
        }
        self.record_unconsumed_temporary(expr);
    }

    /// Record the convention `site` binds its one run-time operand
    /// `source` by, where no selected signature records it: a `not` over a
    /// struct or a `Boolable` type parameter binds its operand as its
    /// `__bool__`'s receiver or the `read` value of upstream's
    /// `Bool.__init__[T: Boolable]`, a copy construction its `copy`
    /// source as the copy initializer's `read` parameter, and a builtin
    /// standing for a library function (`input`, `abort`) its argument as
    /// that function's `read` parameter
    /// ([`SemanticAdjustment::CallConventions`]). The operand's slot writes
    /// only under a `mut`, `var`, or `deinit` receiver: a `ref self` names
    /// no declared origin here.
    pub(super) fn record_operand_convention(
        &self,
        site: &SourceSpan,
        source: mojito_checked::checked::CheckedCallArgumentSource,
        convention: Option<ArgConvention>,
        by_address: bool,
    ) {
        self.record_at(site.clone()).call_conventions =
            Some(vec![mojito_checked::checked::CheckedArgumentConvention {
                source,
                convention,
                by_address,
                writes: matches!(
                    convention,
                    Some(ArgConvention::Mut | ArgConvention::Var | ArgConvention::Deinit)
                ),
            }]);
    }

    /// Record a temporary the enclosing body owns but cannot destroy — a
    /// reference result is none: a call result or display whose type's
    /// [`Self::destruction_verdict`] denies its implicit destruction.
    pub(super) fn record_linear_temporary(&self, expr: &Expr, ty: &Ty) {
        if !matches!(
            expr.kind,
            ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Invoke { .. }
                | ExprKind::ListLit(_)
                | ExprKind::TupleLit(_)
        ) {
            return;
        }
        // A variant's `unsafe_get[T]()` projects its payload in place, as
        // upstream's `ref [self] T` result does.
        if !self.is_variant_projection(expr)
            && let Some(linear) = self.destruction_verdict(ty)
            && self.infer_reference_value(expr).is_none()
        {
            self.linear_temporaries
                .borrow_mut()
                .insert(expr.source_span(), linear);
        }
    }

    /// Record the verdict of the binding introduced at `span` with type
    /// `ty`: linear, or `Deinitable` (no entry).
    pub(super) fn record_binding_verdict(&self, span: SourceSpan, ty: &Ty) {
        let verdict = self.destruction_verdict(ty);
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        match verdict {
            Some(linear) => deletability.bindings.insert(span, linear),
            None => deletability.bindings.remove(&span),
        };
    }

    /// Record `verdict` for the parameter declared at `site`.
    pub(super) fn record_declaration_verdict(
        &self,
        site: mojito_checked::checked::AnnotationSite,
        verdict: Option<crate::explicit_destroy::Linear>,
    ) {
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        match verdict {
            Some(linear) => deletability.declarations.insert(site, linear),
            None => deletability.declarations.remove(&site),
        };
    }

    /// Reject a write through a reference (`ref r = x; r = v`, or
    /// `same(x) = v` through a reference-returning call) over a referent the
    /// body cannot implicitly destroy: the overwritten value is abandoned, as
    /// upstream. `target` names the written place in the diagnostic.
    pub(super) fn check_overwritten_referent(
        &self,
        target: &str,
        referent: &Ty,
    ) -> Result<(), TypeError> {
        let Some(linear) = self.destruction_verdict(referent) else {
            return Ok(());
        };
        let (var, message) = (target.to_string(), linear.message);
        Err(if linear.archetype {
            TypeError::LinearAbandoned { var, message }
        } else {
            TypeError::Abandoned { var, message }
        })
    }

    /// How `source^` transfers, by upstream's one ordered rule
    /// (`UnaryOpNode::emitTransfer`): a compile-time binding, or a field
    /// chain off one, is a parameter expression and is rejected; an owned
    /// rvalue passes through; a trivial register value is copied; any other
    /// source moves out of the root its field projections reach, which must
    /// be tracked storage (`findUnderlyingValueFromField`) and mutable. A
    /// capture held by a mutable reference is indirect, and cannot be
    /// consumed (`checkConsume`).
    pub(super) fn transfer_verdict(
        &self,
        source: &Expr,
        ty: &Ty,
    ) -> Result<TransferVerdict, TypeError> {
        use mojito_ast::ast::CaptureKind;
        if self.is_parameter_read(source) {
            return Err(TypeError::ParameterTransfer);
        }
        // A call's reference result is an lvalue no tracked root holds; a
        // trivial register value read through it is still copied.
        let trivial = self.is_trivial_register_passable(ty);
        if !trivial
            && self
                .node_facts
                .borrow()
                .get(&source.source_span())
                .and_then(|facts| facts.adjustment.as_deref())
                .is_some_and(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                    )
                })
        {
            return Err(TypeError::UntrackedTransfer);
        }
        if !self.designates_place(source) {
            return Ok(TransferVerdict::PassThrough);
        }
        if trivial {
            return Ok(TransferVerdict::TrivialCopy);
        }
        let root = match self.transfer_root(source)? {
            TransferRoot::Temporary => return Ok(TransferVerdict::PlaceMove),
            TransferRoot::Binding(root) => root,
        };
        // A closure's declaration owns its storage, which moves out of it.
        if super::is_closure_storage(ty) || self.is_compile_time_binding(root) {
            return Ok(TransferVerdict::PlaceMove);
        }
        let Some(binding) = self.lookup(root) else {
            return Ok(TransferVerdict::PassThrough);
        };
        if matches!(binding.kind(), TyKind::Ref(_))
            || self
                .lookup_owner(root)
                .is_some_and(|owner| self.reference_binders.contains(&owner))
        {
            return Err(TypeError::UntrackedTransfer);
        }
        let (mutable, indirect) = match self.capture_kind(root) {
            Some(CaptureKind::Copy | CaptureKind::Move) => (true, false),
            Some(CaptureKind::Imm) => (false, true),
            Some(CaptureKind::Mut | CaptureKind::Ref) => (self.is_binding_mutable(root), true),
            None => (self.is_binding_mutable(root), false),
        };
        if !mutable {
            return Err(TypeError::ImmutableTransfer(root.to_string()));
        }
        if indirect {
            return Err(TypeError::IndirectTransfer);
        }
        Ok(TransferVerdict::PlaceMove)
    }

    /// Whether `expr` reads a compile-time binding (a value parameter, a
    /// `comptime for` variable): the binding itself (`q`) or a pure field
    /// chain off it (`q.i.v`), a parameter expression rather than storage.
    /// A module constant bound to a requested value, or folded to a closed
    /// attribute, is one too, as is a local binding of an application: it
    /// has no storage, and each read materializes it. So is the enclosing
    /// struct's pack (`Self.vs`, `Self.Ts`), its visible `ParameterList` or
    /// `TypeList` binder.
    pub(super) fn is_parameter_read(&self, expr: &Expr) -> bool {
        if let ExprKind::Member { object, field } = &expr.kind
            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
            && self.self_pack_visible(field).is_some()
        {
            return true;
        }
        field_chain_root(expr).is_some_and(|root| {
            root != "Self"
                && (self.is_compile_time_binding(root)
                    || self.is_value_parameter(root)
                    || self.local_comptime_application(root).is_some()
                    || (self.binding_scope(root) == Some(0)
                        && (self.local_comptime_parameters[0].contains_key(root)
                            || self.installs(interface::Table::ComptimeParameters, root)
                            || self.module_values.contains_key(root))))
        })
    }

    /// Whether `binder` is one of the enclosing declarations' own type
    /// parameters. The same `TyKind::Param` at a concrete call site is the
    /// erased-dispatch spelling of a solved argument instead.
    pub(super) fn is_own_parameter(&self, binder: &mojito_types::param_expr::ParamRef) -> bool {
        self.param_scopes.borrow().types.iter().any(|scope| {
            scope
                .values()
                .any(|declared| matches!(declared.kind(), TyKind::Param { binder: own, .. } if own == binder))
        })
    }

    /// Whether the check recorded `expr` as a Variant payload projection
    /// (`v[Int]`, `v.unsafe_get[T]()`): a place inside the variant, whatever
    /// the bracket's spelling.
    pub(super) fn is_variant_projection(&self, expr: &Expr) -> bool {
        self.has_fact(&expr.source_span(), |facts| {
            matches!(
                facts.adjustment.as_deref(),
                Some(mojito_checked::checked::SemanticAdjustment::VariantProject { .. })
            )
        })
    }

    /// What `source`'s field projections reach: a named binding, or a
    /// temporary (a call result, a by-value subscript). Any other subscript
    /// yields a reference, which reaches no tracked root.
    fn transfer_root<'e>(&self, source: &'e Expr) -> Result<TransferRoot<'e>, TypeError> {
        match &source.kind {
            ExprKind::Identifier(name) => Ok(TransferRoot::Binding(name)),
            ExprKind::Member { object, .. } => self.transfer_root(object),
            _ if let Some(target) = self.walrus_place(source) => self.transfer_root(target),
            _ if self.designates_place(source) && !self.is_variant_projection(source) => {
                Err(TypeError::UntrackedTransfer)
            }
            _ => Ok(TransferRoot::Temporary),
        }
    }

    /// Whether `expr` is a subscript whose selected getter returns by value.
    fn is_param_value_read(&self, expr: &Expr) -> bool {
        matches!(expr.kind, ExprKind::Identifier(_) | ExprKind::Member { .. })
            && self.has_fact(&expr.source_span(), |facts| {
                matches!(
                    facts.adjustment.as_deref(),
                    Some(mojito_checked::checked::SemanticAdjustment::ParamValue {
                        materialized: None,
                        ..
                    })
                )
            })
    }

    fn is_owned_subscript(&self, expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Index { .. } | ExprKind::MultiIndex { .. }
        ) && self.has_fact(&expr.source_span(), |facts| {
            facts
                .selected_call
                .as_ref()
                .is_some_and(|call| call.reference_result.is_none())
        })
    }
}

/// The argument positions a call's positional `*args` collector gathers,
/// and whether it owns them (`var *xs`) or reads them (`*xs`).
#[derive(Clone, Copy)]
pub(super) struct Gathered<'a> {
    pub(super) positions: &'a [usize],
    pub(super) owning: bool,
}

impl<'a> Gathered<'a> {
    pub(super) const NONE: Self = Self {
        positions: &[],
        owning: false,
    };

    /// A collector with convention `convention` over `positions`: a read
    /// (`None`/`read`) or owning (`var`/`deinit`) one gathers them, and a
    /// `mut` or `ref` one is borrowed apart.
    pub(super) const fn of_convention(
        convention: Option<ArgConvention>,
        positions: &'a [usize],
    ) -> Self {
        match convention {
            None | Some(ArgConvention::Imm) => Self {
                positions,
                owning: false,
            },
            Some(ArgConvention::Var | ArgConvention::Deinit) => Self {
                positions,
                owning: true,
            },
            _ => Self::NONE,
        }
    }

    /// The positions a read collector gathers; none for an owning one.
    const fn read(self) -> &'a [usize] {
        if self.owning { &[] } else { self.positions }
    }
}

/// What each parameter's declaration says of the argument bound to it
/// ([`Checker::declared_slots`]).
pub(super) struct DeclaredSlots {
    /// See [`mojito_checked::checked::CheckedArgumentConvention::by_address`].
    pub(super) by_address: Vec<bool>,
    /// See [`mojito_checked::checked::CheckedArgumentConvention::writes`].
    pub(super) writes: Vec<bool>,
}

pub(super) fn is_place_expr(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. }
    ) || pointer_offset_keyword_subscript(e).is_some()
        || walrus_destination(e).is_some()
}

/// The destination the walrus `e` yields borrowed after its store, as
/// upstream's `emitWalrus` yields a memory lvalue's: its target, when that
/// is a place — a name, a field, a subscript, a pointer dereference, or a
/// call's mutable reference result. A
/// subscript stored through its `__setitem__` is the target's getter read
/// here; [`Checker::walrus_place`] leaves it out. `None` for any other
/// expression.
pub(super) fn walrus_destination(e: &Expr) -> Option<&Expr> {
    match &e.kind {
        ExprKind::Named { target, .. }
            if is_place_expr(target)
                || matches!(
                    target.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                ) =>
        {
            Some(target)
        }
        _ => None,
    }
}

/// The pointer keyword subscript (`p[unsafe_offset=i]`, current Mojo's
/// offset-dereference spelling) as its pointer object and offset expression.
/// It is a place exactly like the positional `p[i]`; every other keyword
/// subscript is a nominal `__getitem__` call.
pub(super) fn pointer_offset_keyword_subscript(e: &Expr) -> Option<(&Expr, &Expr)> {
    match &e.kind {
        ExprKind::MultiIndex { object, args } => match args.as_slice() {
            [mojito_ast::ast::SubscriptArg::Keyword { name, value }] if name == "unsafe_offset" => {
                Some((object, value))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The root variable of a place expression (`p` for `p`, `p.a.b`, `p.items[i]`),
/// or `None` if the expression isn't rooted at a variable. A `mut`/shared borrow of
/// a place borrows its root, so the borrow checker keys on this.
/// Mojo's within-call mutable-XOR-shared rule is **place-sensitive**
/// (field-aware). An argument accesses its place either **exclusively** (a
/// `mut`/effectively mutable `ref` borrow, or a `^` move) or **shared** (an
/// immutable/default borrow).
/// Any number of shared accesses to overlapping places is fine, but an exclusive
/// access requires no *overlapping* place elsewhere in the call — so `f(mut a, a)`,
/// `f(mut a, mut a)`, `f(a, a^)`, and `f(mut p, p.a)` are rejected, while
/// `f(mut p.a, mut p.b)` (disjoint fields) is allowed. mojito's borrows are
/// also checked by persistent cross-block loan dataflow; this routine handles
/// conflicts among actuals evaluated at one call site.
pub(super) fn check_call_aliasing(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    copied_reads: &[bool],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Result<(), TypeError> {
    // Each place argument's access: its full place (root + projection path) and
    // whether it is *exclusive* (a `mut`/`ref` borrow, or a `^` move).
    let mut accesses: Vec<(&str, Vec<PlaceSeg>, bool, bool)> = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        let arg = match slot {
            ArgSlot::Positional(p) => &args[*p],
            ArgSlot::Keyword(k) => &kwargs[*k].value,
            ArgSlot::Default => continue,
        };
        let (place, exclusive) = match &arg.kind {
            ExprKind::Transfer(inner) => (place_path(inner), true),
            _ => (
                place_path(arg),
                matches!(
                    conventions.get(i),
                    Some(Some(ArgConvention::Mut | ArgConvention::Ref))
                ),
            ),
        };
        if let Some((root, path)) = place {
            accesses.push((
                root,
                path,
                exclusive,
                copied_reads.get(i).copied().unwrap_or(false),
            ));
        }
    }
    // Mutable-XOR-shared, **place-sensitive**: two accesses to the *same variable*
    // conflict only if their places overlap (a prefix relationship) and at least one
    // is exclusive. So `f(mut p.a, mut p.b)` is fine (disjoint fields), while
    // `f(mut p.a, p.a)` and `f(mut p, p.a)` are rejected.
    for i in 0..accesses.len() {
        for j in (i + 1)..accesses.len() {
            let (ra, pa, ea, ca) = &accesses[i];
            let (rb, pb, eb, cb) = &accesses[j];
            let live_alias_conflict = (*ea && !*cb) || (*eb && !*ca);
            if ra == rb && live_alias_conflict && places_overlap(pa, pb) {
                return Err(TypeError::AliasingViolation {
                    var: ra.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Extend the within-call alias check to a method receiver. A mutable or
/// mutability-preserving reference receiver participates in the same exclusive
/// access set as `mut`/`ref` arguments; otherwise `value[value.field]` could
/// smuggle an overlapping mutable argument through subscript syntax even though
/// the equivalent ordinary method call is exclusive on `self`.
pub(super) fn check_receiver_aliasing(
    receiver: &Expr,
    receiver_convention: Option<ArgConvention>,
    slots: &[ArgSlot],
    copied_reads: &[bool],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Result<(), TypeError> {
    if !matches!(
        receiver_convention,
        Some(ArgConvention::Mut | ArgConvention::Ref)
    ) {
        return Ok(());
    }
    let Some((receiver_root, receiver_path)) = place_path(receiver) else {
        return Ok(());
    };
    for (index, slot) in slots.iter().enumerate() {
        let argument = match slot {
            ArgSlot::Positional(position) => &args[*position],
            ArgSlot::Keyword(position) => &kwargs[*position].value,
            ArgSlot::Default => continue,
        };
        let place = match &argument.kind {
            ExprKind::Transfer(inner) => place_path(inner),
            _ => place_path(argument),
        };
        let Some((argument_root, argument_path)) = place else {
            continue;
        };
        let argument_is_copied = copied_reads.get(index).copied().unwrap_or(false);
        if receiver_root == argument_root
            && !argument_is_copied
            && places_overlap(&receiver_path, &argument_path)
        {
            return Err(TypeError::AliasingViolation {
                var: receiver_root.to_string(),
            });
        }
    }
    Ok(())
}

/// Owned temporaries a call binds to read parameters: a non-place argument
/// (a call result, a by-value subscript, a construction, a literal an
/// implicit conversion turns into a nominal value, per `designates_place`)
/// whose effective convention is a shared read. The
/// callee only borrows such a temporary, so the caller destroys it once the
/// call returns; a `var`/`deinit` slot moves the temporary into the callee,
/// which destroys it, and records nothing here.
pub(super) fn read_temporary_arguments(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    collected: &[usize],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
    designates_place: impl Fn(&Expr) -> bool,
) -> Vec<SourceSpan> {
    slots
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            matches!(
                conventions.get(*index),
                Some(None | Some(ArgConvention::Imm))
            )
        })
        .filter_map(|(_, slot)| match slot {
            ArgSlot::Positional(position) => Some(&args[*position]),
            ArgSlot::Keyword(position) => Some(&kwargs[*position].value),
            ArgSlot::Default => None,
        })
        .chain(collected_read_arguments(collected, args))
        .filter(|arg| !designates_place(arg) && !matches!(arg.kind, ExprKind::Transfer(_)))
        .map(Expr::source_span)
        .collect()
}

/// The conventions a call binds its run-time arguments by
/// ([`Checker::record_call_conventions`]).
pub(super) fn argument_conventions(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    declared: &DeclaredSlots,
    gathered: Gathered<'_>,
) -> Vec<mojito_checked::checked::CheckedArgumentConvention> {
    use mojito_checked::checked::{CheckedArgumentConvention, CheckedCallArgumentSource};
    let collector = gathered.owning.then_some(ArgConvention::Var);
    slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let source = match slot {
                ArgSlot::Positional(position) => CheckedCallArgumentSource::Positional(*position),
                ArgSlot::Keyword(position) => CheckedCallArgumentSource::Keyword(*position),
                ArgSlot::Default => return None,
            };
            Some(CheckedArgumentConvention {
                source,
                convention: conventions.get(index).copied().flatten(),
                by_address: declared.by_address.get(index).copied().unwrap_or(false),
                writes: declared.writes.get(index).copied().unwrap_or(false),
            })
        })
        .chain(
            gathered
                .positions
                .iter()
                .map(|position| CheckedArgumentConvention {
                    source: CheckedCallArgumentSource::Positional(*position),
                    convention: collector,
                    by_address: false,
                    writes: gathered.owning,
                }),
        )
        .collect()
}

/// How a read argument names storage ([`borrowable_read_arguments`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadPlace {
    /// A value: a temporary, a literal, an operator result.
    Value,
    /// A place: a binding, or a field or subscript chain rooted at one.
    Place,
    /// A place a reference result names: an accessor's, or a
    /// reference-returning call's (`o.value()`), which the parameter binds
    /// as upstream binds a `read` argument to the returned reference.
    ReferenceResult,
}

/// Read-convention place arguments a call may bind by borrow instead of the
/// implicit `__copyinit__` read. An argument qualifies when its effective
/// convention is a shared read, it names a place, and no exclusive access
/// (a `mut`/`ref` slot, a `^` transfer, or a `mut`/`ref` receiver) overlaps
/// that place in the same call — the overlap case must keep the implicit copy
/// that lets `check_call_aliasing` accept it, never a shallow alias of storage
/// the callee mutates.
pub(super) fn borrowable_read_arguments(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    collected: &[usize],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
    receiver: Option<(&Expr, Option<ArgConvention>)>,
    read_place: impl Fn(&Expr) -> ReadPlace,
) -> Vec<SourceSpan> {
    let argument = |slot: &ArgSlot| match slot {
        ArgSlot::Positional(position) => Some(&args[*position]),
        ArgSlot::Keyword(position) => Some(&kwargs[*position].value),
        ArgSlot::Default => None,
    };
    let read_slot = |index: usize| {
        matches!(
            conventions.get(index),
            Some(None | Some(ArgConvention::Imm))
        )
    };
    let mut exclusive: Vec<(&str, Vec<PlaceSeg>)> = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        let Some(arg) = argument(slot) else { continue };
        let place = match &arg.kind {
            // A `^` into a read slot lends its place rather than moving it.
            ExprKind::Transfer(_) if read_slot(index) => None,
            ExprKind::Transfer(inner) => place_path(inner),
            _ if matches!(
                conventions.get(index),
                Some(Some(ArgConvention::Mut | ArgConvention::Ref))
            ) =>
            {
                place_path(arg)
            }
            _ => None,
        };
        if let Some(place) = place {
            exclusive.push(place);
        }
    }
    if let Some((object, convention)) = receiver
        && matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
        && let Some(place) = place_path(object)
    {
        exclusive.push(place);
    }
    let mut borrowable = Vec::new();
    let reads = slots
        .iter()
        .enumerate()
        .filter(|(index, _)| read_slot(*index))
        .filter_map(|(_, slot)| argument(slot))
        .chain(collected_read_arguments(collected, args));
    for arg in reads {
        // A `^` into a read slot does not consume its source, as in current
        // Mojo: the transfer node itself is recorded, and its place borrows
        // like a plain read argument.
        let arg = match &arg.kind {
            ExprKind::Transfer(inner) => {
                borrowable.push(arg.source_span());
                inner.as_ref()
            }
            _ => arg,
        };
        let kind = read_place(arg);
        if kind == ReadPlace::Value {
            continue;
        }
        // A reference-returning call (`o.value()`), or an accessor's
        // reference result over a temporary key (`d[String("k")]`), names
        // no place path, yet the parameter reads its referent through the
        // handle rather than copying it out; the origin exclusivity rule
        // judges its overlap with a `mut` argument.
        let Some((root, path)) = place_path(arg) else {
            if kind == ReadPlace::ReferenceResult {
                borrowable.push(arg.source_span());
            }
            continue;
        };
        if exclusive
            .iter()
            .any(|(er, ep)| *er == root && places_overlap(ep, &path))
        {
            continue;
        }
        borrowable.push(arg.source_span());
    }
    borrowable
}

/// A `^` transfer produces a value, not a place, so it binds no `mut` or
/// `ref` parameter, and a walrus yields its destination borrowed,
/// immutably, so it binds no `mut` one.
pub(super) fn reject_transfer_into_mutable(
    func: &str,
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Result<(), TypeError> {
    for (index, slot) in slots.iter().enumerate() {
        let argument = match slot {
            ArgSlot::Positional(position) => &args[*position],
            ArgSlot::Keyword(position) => &kwargs[*position].value,
            ArgSlot::Default => continue,
        };
        let reason = match (&argument.kind, conventions.get(index)) {
            (ExprKind::Transfer(_), Some(Some(ArgConvention::Mut | ArgConvention::Ref))) => {
                "a transferred ('^') value is not a place"
            }
            (ExprKind::Named { .. }, Some(Some(ArgConvention::Mut))) => {
                "a walrus yields its destination borrowed"
            }
            _ => continue,
        };
        return Err(TypeError::BadCall {
            func: func.to_string(),
            reason: format!("value passed to a mutable argument must be mutable; {reason}"),
        });
    }
    Ok(())
}

/// One step of a place's projection path (used by the place-sensitive borrow
/// check). A dynamic `Index` is treated conservatively — it may alias any index.
pub(super) enum PlaceSeg {
    Field(String),
    Index,
}

/// A place expression's root variable and projection path (root → leaf), or `None`
/// if it isn't rooted at a variable.
pub(super) fn place_path(e: &Expr) -> Option<(&str, Vec<PlaceSeg>)> {
    fn go<'a>(e: &'a Expr, path: &mut Vec<PlaceSeg>) -> Option<&'a str> {
        if let Some(target) = walrus_destination(e) {
            return go(target, path);
        }
        match &e.kind {
            ExprKind::Identifier(n) => Some(n),
            ExprKind::Member { object, field } => {
                let r = go(object, path)?;
                path.push(PlaceSeg::Field(field.clone()));
                Some(r)
            }
            ExprKind::Index { object, .. } => {
                let r = go(object, path)?;
                path.push(PlaceSeg::Index);
                Some(r)
            }
            ExprKind::MultiIndex { .. } => {
                let (object, _) = pointer_offset_keyword_subscript(e)?;
                let r = go(object, path)?;
                path.push(PlaceSeg::Index);
                Some(r)
            }
            _ => None,
        }
    }
    let mut path = Vec::new();
    let root = go(e, &mut path)?;
    Some((root, path))
}

/// Whether two projection paths (of the same root) may refer to overlapping
/// memory: they overlap unless a `Field` step names distinct fields. A dynamic
/// `Index` conservatively may alias, so it never proves disjointness.
fn places_overlap(a: &[PlaceSeg], b: &[PlaceSeg]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if let (PlaceSeg::Field(fa), PlaceSeg::Field(fb)) = (x, y)
            && fa != fb
        {
            return false;
        }
    }
    true
}

/// The arguments a read `*args` collector gathers one element each from. A
/// forwarded `*pack` is the collector's whole value, not an element.
fn collected_read_arguments<'a>(
    collected: &'a [usize],
    args: &'a [Expr],
) -> impl Iterator<Item = &'a Expr> {
    collected
        .iter()
        .map(|position| &args[*position])
        .filter(|arg| !matches!(arg.kind, ExprKind::Spread(_)))
}

/// A method's temporary receiver whose **declared** convention is a read:
/// the method only borrows it, so the caller destroys it once the call
/// returns, as a read argument's ([`read_temporary_arguments`]). A
/// `ref self` receiver is not one even where its effective convention is
/// a read, and a `^` receiver lends its place
/// ([`Checker::check_consumed_receiver`]).
fn read_temporary_receiver(
    receiver: Option<(&Expr, Option<ArgConvention>)>,
    designates_place: impl Fn(&Expr) -> bool,
) -> Option<SourceSpan> {
    receiver
        .filter(|(receiver, convention)| {
            matches!(convention, None | Some(ArgConvention::Imm))
                && !designates_place(receiver)
                && !matches!(receiver.kind, ExprKind::Transfer(_))
        })
        .map(|(receiver, _)| receiver.source_span())
}

/// A method's place receiver whose **declared** convention is a read:
/// upstream loads a register-passable one at call setup, as it does a
/// `read` argument, so its owner's last use is that load. A `^` receiver
/// lends its place ([`Checker::check_consumed_receiver`]).
fn read_place_receiver(
    receiver: Option<(&Expr, Option<ArgConvention>)>,
    designates_place: impl Fn(&Expr) -> bool,
) -> Option<SourceSpan> {
    receiver
        .filter(|(receiver, convention)| {
            matches!(convention, None | Some(ArgConvention::Imm))
                && designates_place(receiver)
                && !matches!(receiver.kind, ExprKind::Transfer(_))
        })
        .map(|(receiver, _)| receiver.source_span())
}

/// The root a transfer's field projections reach.
enum TransferRoot<'e> {
    Binding(&'e str),
    Temporary,
}

/// The binding a plain place expression (a name, or a field chain over one)
/// is rooted at.
fn field_chain_root(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Member { object, .. } => field_chain_root(object),
        _ => None,
    }
}

/// The arguments a call binds to its `mut` and `ref` parameters.
fn exclusive_reference_arguments<'a>(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
) -> Vec<&'a Expr> {
    slots
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            matches!(
                conventions.get(*index),
                Some(Some(ArgConvention::Mut | ArgConvention::Ref))
            )
        })
        .filter_map(|(_, slot)| match slot {
            ArgSlot::Positional(position) => Some(&args[*position]),
            ArgSlot::Keyword(position) => Some(&kwargs[*position].value),
            ArgSlot::Default => None,
        })
        .collect()
}
