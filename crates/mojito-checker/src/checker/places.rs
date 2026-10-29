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
    /// ([`read_temporary_receiver`]). It also records the arguments
    /// and the receiver that bind an owned parameter, among them the
    /// elements an owning (`var`) collector gathers
    /// ([`forwarded_arguments`]).
    pub(super) fn record_argument_borrows(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        gathered: Gathered<'_>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        receiver: Option<(&Expr, Option<ArgConvention>)>,
    ) {
        let designates_place = |arg: &Expr| self.designates_place(arg);
        let read_place = |arg: &Expr| match self.designates_place(arg) {
            false => ReadPlace::Value,
            true if self.infer_reference_value(arg).is_some() => ReadPlace::ReferenceResult,
            true => ReadPlace::Place,
        };
        let collected = gathered.read();
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
        let owned_receiver = receiver
            .filter(|(_, convention)| owns_argument(*convention))
            .map(|(receiver, _)| receiver.source_span());
        let owned_elements = gathered
            .owned()
            .iter()
            .map(|position| args[*position].source_span());
        for span in forwarded_arguments(slots, conventions, args, kwargs)
            .into_iter()
            .chain(owned_elements)
            .chain(owned_receiver)
        {
            self.record_at(span).forwarded_argument = true;
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
    /// storage read stay places.
    pub(super) fn designates_place(&self, expr: &Expr) -> bool {
        is_place_expr(expr) && !self.is_owned_subscript(expr)
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

    /// Record an operand an operator's dunder binds to an owned (`var`/
    /// `deinit`) receiver or parameter, as [`forwarded_arguments`] records
    /// a call's.
    pub(super) fn record_forwarded_operand(&self, expr: &Expr) {
        self.record_at(expr.source_span()).forwarded_argument = true;
    }

    /// Record a discarded `^` transfer (`_ = x^`, or `x^` as a statement):
    /// the discard takes the transferred storage and destroys it in place,
    /// as an owned parameter takes it, with no move initializer.
    pub(super) fn record_discarded_transfer(&self, expr: &Expr) {
        if matches!(expr.kind, ExprKind::Transfer(_)) {
            self.record_forwarded_operand(expr);
        }
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
        let projection = self.has_fact(&expr.source_span(), |facts| {
            matches!(
                facts.adjustment.as_deref(),
                Some(mojito_checked::checked::SemanticAdjustment::VariantProject { .. })
            )
        });
        if !projection
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
        if !self.designates_place(source) {
            return Ok(TransferVerdict::PassThrough);
        }
        if self.is_trivial_register_passable(ty) {
            return Ok(TransferVerdict::TrivialCopy);
        }
        let root = match self.transfer_root(source)? {
            TransferRoot::Temporary => return Ok(TransferVerdict::PlaceMove),
            TransferRoot::Binding(root) => root,
        };
        // A closure's declaration owns its storage, which moves out of it.
        let closure_storage = matches!(
            ty.callable().map(|func| &func.environment),
            Some(mojito_types::origin::CallableEnvironment::Storage(_))
        );
        if closure_storage || self.is_compile_time_binding(root) {
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
    /// has no storage, and each read materializes it.
    pub(super) fn is_parameter_read(&self, expr: &Expr) -> bool {
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
        self.tparams.iter().any(|scope| {
            scope
                .values()
                .any(|declared| matches!(declared.kind(), TyKind::Param { binder: own, .. } if own == binder))
        })
    }

    /// What `source`'s field projections reach: a named binding, or a
    /// temporary (a call result, a by-value subscript, an element of the
    /// private `Tuple` storage, which is a tracked owned slot until R341).
    /// Any other subscript yields a reference, which reaches no tracked
    /// root.
    fn transfer_root<'e>(&self, source: &'e Expr) -> Result<TransferRoot<'e>, TypeError> {
        match &source.kind {
            ExprKind::Identifier(name) => Ok(TransferRoot::Binding(name)),
            ExprKind::Member { object, .. } => self.transfer_root(object),
            ExprKind::Index { object, .. }
                if matches!(
                    self.infer(object).as_ref().map(Ty::kind),
                    Ok(TyKind::Tuple(_))
                ) =>
            {
                Ok(TransferRoot::Temporary)
            }
            _ if self.designates_place(source)
                && !matches!(source.kind, ExprKind::TypeApply { .. }) =>
            {
                Err(TypeError::UntrackedTransfer)
            }
            _ => Ok(TransferRoot::Temporary),
        }
    }

    /// Whether `expr` is a subscript whose selected getter returns by value.
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

    /// The positions an owning collector gathers; none for a read one.
    const fn owned(self) -> &'a [usize] {
        if self.owning { self.positions } else { &[] }
    }
}

/// Whether a parameter convention owns its argument: `var` or `deinit`.
pub(super) const fn owns_argument(convention: Option<ArgConvention>) -> bool {
    matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
}

pub(super) fn is_place_expr(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Identifier(_)
            | ExprKind::Member { .. }
            | ExprKind::Index { .. }
            | ExprKind::TypeApply { .. }
    ) || pointer_offset_keyword_subscript(e).is_some()
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

/// Arguments a call binds to regular owned (`var`/`deinit`) parameters,
/// which take the caller's storage: a `^` transfer among them hands its
/// slot over without the move initializer. An owning collector's elements
/// are added by [`Checker::record_argument_borrows`].
pub(super) fn forwarded_arguments(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Vec<SourceSpan> {
    slots
        .iter()
        .enumerate()
        .filter(|(index, _)| owns_argument(conventions.get(*index).copied().flatten()))
        .filter_map(|(_, slot)| match slot {
            ArgSlot::Positional(position) => Some(&args[*position]),
            ArgSlot::Keyword(position) => Some(&kwargs[*position].value),
            ArgSlot::Default => None,
        })
        .map(Expr::source_span)
        .collect()
}

/// How a read argument names storage ([`borrowable_read_arguments`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadPlace {
    /// A value: a temporary, a literal, an operator result.
    Value,
    /// A place: a binding, or a field or subscript chain rooted at one.
    Place,
    /// A place an accessor's reference result names.
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
        // An accessor's reference result over a temporary key
        // (`d[String("k")]`) names no place path, yet the parameter reads
        // its referent through the handle rather than copying it out.
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

/// A `^` transfer produces a value, not a place, so it cannot bind a `mut` or
/// `ref` parameter.
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
        if matches!(argument.kind, ExprKind::Transfer(_))
            && matches!(
                conventions.get(index),
                Some(Some(ArgConvention::Mut | ArgConvention::Ref))
            )
        {
            return Err(TypeError::BadCall {
                func: func.to_string(),
                reason: "value passed to a mutable argument must be mutable; a transferred \
                         ('^') value is not a place"
                    .to_string(),
            });
        }
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
