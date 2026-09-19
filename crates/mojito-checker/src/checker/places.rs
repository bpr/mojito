//! Place construction, overlap, mutability, and origin derivation for checking.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
    /// ([`read_temporary_arguments`]).
    pub(super) fn record_argument_borrows(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        receiver: Option<(&Expr, Option<ArgConvention>)>,
    ) {
        self.borrowed_read_call_places
            .borrow_mut()
            .extend(borrowable_read_arguments(
                slots,
                conventions,
                args,
                kwargs,
                receiver,
            ));
        let read = read_temporary_arguments(slots, conventions, args, kwargs);
        self.unconsumed_temporaries
            .borrow_mut()
            .extend(read.iter().cloned());
        self.read_temporary_arguments.borrow_mut().extend(read);
    }

    /// Record an expression whose value nothing takes ownership of: a
    /// temporary a callee only borrows, or a discarded statement expression.
    /// A linear one is abandoned there ([`Self::record_linear_temporary`]).
    pub(super) fn record_unconsumed_temporary(&self, expr: &Expr) {
        if !is_place_expr(expr) && !matches!(expr.kind, ExprKind::Transfer(_)) {
            self.unconsumed_temporaries
                .borrow_mut()
                .insert(expr.source_span());
        }
    }

    /// Record a call result the enclosing body owns but cannot destroy: its
    /// type is one of that body's own type parameters, whose bounds do not
    /// prove `Deinitable`. The parameter must be in scope here — the same
    /// `Ty::Param` at a concrete call site is the erased-dispatch spelling of
    /// a solved argument, not a linear value.
    pub(super) fn record_linear_temporary(&self, expr: &Expr, ty: &Ty) {
        let Ty::Param { name, .. } = ty else {
            return;
        };
        let call = matches!(
            expr.kind,
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Invoke { .. }
        );
        let own_parameter = self
            .tparams
            .iter()
            .any(|scope| scope.contains_key(name.as_str()));
        if call && own_parameter && !self.is_deinitable(ty) {
            self.linear_temporaries
                .borrow_mut()
                .insert(expr.source_span());
        }
    }
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
/// (a call result, a construction, a literal an implicit conversion turns
/// into a nominal value) whose effective convention is a shared read. The
/// callee only borrows such a temporary, so the caller destroys it once the
/// call returns; a `var`/`deinit` slot moves the temporary into the callee,
/// which destroys it, and records nothing here.
pub(super) fn read_temporary_arguments(
    slots: &[ArgSlot],
    conventions: &[Option<ArgConvention>],
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
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
        .filter(|arg| !is_place_expr(arg) && !matches!(arg.kind, ExprKind::Transfer(_)))
        .map(Expr::source_span)
        .collect()
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
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
    receiver: Option<(&Expr, Option<ArgConvention>)>,
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
    for (index, slot) in slots.iter().enumerate() {
        if !read_slot(index) {
            continue;
        }
        let Some(arg) = argument(slot) else { continue };
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
        if !is_place_expr(arg) {
            continue;
        }
        let Some((root, path)) = place_path(arg) else {
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
