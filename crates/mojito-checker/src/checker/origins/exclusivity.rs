//! Argument exclusivity over carried origins: a call may not hand one
//! callee a mutable path to some storage through one argument and any path
//! to the same storage through another, whether the path is the argument's
//! own place (a `mut` parameter) or an origin its declared parameter type
//! names (`RefBox[o]`, `List[RefBox[o]]`, a `Pointer[T, o]`), spelled or
//! received through a type argument (`value: T` bound to a `Span[Int,
//! origin_of(xs)]`). Current Mojo rejects `stash(sink,
//! RefBox(Pointer(to=view)))` over `mut sink: List[RefBox[o]], var box:
//! RefBox[o]` this way, and `l.insert(0, Span(xs))` on a `List[Span[Int,
//! origin_of(xs)]]` too. A callee decorated
//! `@__unsafe_nested_origins_read_only` (`List.append`) reaches every origin
//! its argument types carry immutably, so two such paths do not conflict.
//! Two arguments' *own* places are the syntactic place rule's business
//! (`check_call_aliasing`); this rule judges every pair with a carried origin
//! on at least one side.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::origin::{Mutability, Origin, OriginPlace, PointerOrigin, places_overlap};

/// How a callee reaches the origins its argument types carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::checker) enum NestedOrigins {
    /// As each origin allows: a mutable origin is reached mutably.
    AsDeclared,
    /// `@__unsafe_nested_origins_read_only`: immutably, as is a `ref`
    /// argument's own place.
    ReadOnly,
}

impl NestedOrigins {
    pub(in crate::checker) fn of(decorators: &[mojito_ast::ast::Decorator]) -> Self {
        if mojito_ast::ast::is_nested_origins_read_only(decorators) {
            Self::ReadOnly
        } else {
            Self::AsDeclared
        }
    }

    fn read_only(self) -> bool {
        self == Self::ReadOnly
    }
}

/// What a call's exclusivity is judged against besides its arguments: the
/// callee's name, its parameter names and types, and how it reaches the
/// origins its argument types carry.
pub(in crate::checker) struct ExclusivityCallee<'a> {
    pub(in crate::checker) name: &'a str,
    pub(in crate::checker) parameter_names: &'a [String],
    /// Declared parameter types with the callee's origin binders bound for
    /// this call and its type parameters left abstract.
    pub(in crate::checker) declared: &'a [Ty],
    /// The parameter types this call binds, type parameters substituted.
    pub(in crate::checker) bound: &'a [Ty],
    pub(in crate::checker) nested_origins: NestedOrigins,
}

/// A method call's receiver: its expression and convention, the declared
/// `Self` type with the receiver's origin tail bound, and the receiver's own
/// type.
pub(in crate::checker) struct ExclusivityReceiver<'a> {
    pub(in crate::checker) object: &'a Expr,
    pub(in crate::checker) convention: Option<ArgConvention>,
    pub(in crate::checker) declared: &'a Ty,
    pub(in crate::checker) bound: &'a Ty,
}

/// One argument's access to caller storage: its own place when passed by
/// reference or read by borrow, and the places its type carries.
struct ArgumentAccess {
    name: String,
    own: Option<(OriginPlace, bool)>,
    carried: Vec<(OriginPlace, bool)>,
    /// The callee only reads the places the argument's type carries.
    carried_read_only: bool,
}

impl ArgumentAccess {
    const fn new(name: String, carried_read_only: bool) -> Self {
        Self {
            name,
            own: None,
            carried: Vec::new(),
            carried_read_only,
        }
    }

    fn carry(&mut self, place: &OriginPlace, mutable: bool) {
        let place = place.without_subtree();
        let mutable = mutable && !self.carried_read_only;
        match self.carried.iter_mut().find(|(known, _)| *known == place) {
            Some((_, known_mutable)) => *known_mutable |= mutable,
            None => self.carried.push((place, mutable)),
        }
    }

    /// Whether a mutable path of `self` overlaps any path of `other`,
    /// other than the two arguments' own places.
    fn mutably_overlaps(&self, other: &Self) -> bool {
        let own_mutable = self
            .own
            .iter()
            .filter(|(_, mutable)| *mutable)
            .any(|(place, _)| {
                other
                    .carried
                    .iter()
                    .any(|(theirs, _)| places_overlap(place, theirs))
            });
        let carried_mutable =
            self.carried
                .iter()
                .filter(|(_, mutable)| *mutable)
                .any(|(place, _)| {
                    other
                        .own
                        .iter()
                        .map(|(theirs, _)| theirs)
                        .chain(other.carried.iter().map(|(theirs, _)| theirs))
                        .any(|theirs| places_overlap(place, theirs))
                });
        own_mutable || carried_mutable
    }
}

impl Checker {
    /// The within-call aliasing rules of a free call in order: a transferred
    /// value cannot bind a `mut`/`ref` slot (`reject_transfer_into_mutable`),
    /// then the syntactic place rule (`check_call_aliasing`), then this
    /// module's carried-origin rule.
    pub(in crate::checker) fn check_free_call_aliasing(
        &self,
        callee: &ExclusivityCallee<'_>,
        conventions: &[Option<ArgConvention>],
        copied_reads: &[bool],
        slots: &[ArgSlot],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        crate::checker::places::reject_transfer_into_mutable(
            callee.name,
            slots,
            conventions,
            args,
            kwargs,
        )?;
        crate::checker::places::check_call_aliasing(
            slots,
            conventions,
            copied_reads,
            args,
            kwargs,
        )?;
        self.check_argument_origin_exclusivity(callee, None, conventions, slots, args, kwargs)
    }

    /// A free function's side of the exclusivity rule.
    pub(in crate::checker) fn free_callee<'a>(
        &self,
        name: &'a str,
        parameter_names: &'a [String],
        declared: &'a [Ty],
        bound: &'a [Ty],
    ) -> ExclusivityCallee<'a> {
        ExclusivityCallee {
            name,
            parameter_names,
            declared,
            bound,
            nested_origins: if self.nested_origins_read_only_functions.contains(name) {
                NestedOrigins::ReadOnly
            } else {
                NestedOrigins::AsDeclared
            },
        }
    }

    /// Reject a call two of whose arguments (the receiver included) reach
    /// overlapping caller storage with at least one mutable path, reporting
    /// the first such pair in argument order as current Mojo does.
    ///
    /// An argument carries the origins of its declared parameter type, whose
    /// slots fix their own mutability, and those of the type the call binds
    /// it to, which an origin reaching the callee through a type argument
    /// (`value: T` bound to a `Span[Int, origin_of(xs)]`) appears in only.
    pub(in crate::checker) fn check_argument_origin_exclusivity(
        &self,
        callee: &ExclusivityCallee<'_>,
        receiver: Option<&ExclusivityReceiver<'_>>,
        conventions: &[Option<ArgConvention>],
        slots: &[ArgSlot],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        let mut accesses: Vec<ArgumentAccess> = Vec::new();
        if let Some(receiver) = receiver {
            let mut access =
                ArgumentAccess::new("self".to_string(), callee.nested_origins.read_only());
            access.own = self.own_place(
                receiver.object,
                receiver.convention,
                callee.nested_origins.read_only(),
            );
            self.record_carried_places(&mut access, receiver.declared);
            self.record_carried_places(&mut access, receiver.bound);
            accesses.push(access);
        }
        for (index, slot) in slots.iter().enumerate() {
            let argument = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let mut access = ArgumentAccess::new(
                callee
                    .parameter_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("arg{index}")),
                callee.nested_origins.read_only(),
            );
            access.own = self.own_place(
                argument,
                conventions.get(index).copied().flatten(),
                callee.nested_origins.read_only(),
            );
            for parameter in [callee.declared.get(index), callee.bound.get(index)]
                .into_iter()
                .flatten()
            {
                self.record_carried_places(&mut access, parameter);
            }
            accesses.push(access);
        }
        for (left, first) in accesses.iter().enumerate() {
            for second in &accesses[left + 1..] {
                let first_mutable = first.mutably_overlaps(second);
                let second_mutable = second.mutably_overlaps(first);
                if !first_mutable && !second_mutable {
                    continue;
                }
                let (mutable, other, other_mutable) = if first_mutable {
                    (first, second, second_mutable)
                } else {
                    (second, first, true)
                };
                return Err(TypeError::AliasingArguments {
                    mutable: mutable.name.clone(),
                    other: other.name.clone(),
                    other_mutable,
                    callee: callee.name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// An argument passed by reference (`mut`/`ref`) reaches its own place
    /// mutably, a `ref` one immutably when the callee only reads
    /// (`ref_read_only`); a place argument read by borrow reaches it
    /// immutably; a transferred, consumed, copied, or non-place argument
    /// reaches no place of its own.
    fn own_place(
        &self,
        argument: &Expr,
        convention: Option<ArgConvention>,
        ref_read_only: bool,
    ) -> Option<(OriginPlace, bool)> {
        if matches!(argument.kind, ExprKind::Transfer(_)) {
            return None;
        }
        let place = self.origin_place(argument).ok()?.without_subtree();
        match convention {
            Some(ArgConvention::Mut) => Some((place, true)),
            Some(ArgConvention::Ref) => Some((place, !ref_read_only)),
            Some(ArgConvention::Var | ArgConvention::Deinit | ArgConvention::Out) => None,
            Some(ArgConvention::Imm) | None => {
                let copied = self
                    .infer(argument)
                    .is_ok_and(|ty| self.call_read_is_independent_copy(&ty));
                (!copied).then_some((place, false))
            }
        }
    }

    /// The caller places a declared parameter type names: struct origin
    /// tails (a `mut=True` slot mutably, a `mut=False` slot immutably, a
    /// symbolic `mut=m` slot as the place allows), pointer provenance, and
    /// reference origins, recursing through type arguments and tuples.
    fn record_carried_places(&self, access: &mut ArgumentAccess, ty: &Ty) {
        match ty {
            Ty::Struct(name, arguments) => {
                for argument in arguments {
                    if let TyArg::Ty(inner) = argument {
                        self.record_carried_places(access, inner);
                    }
                }
                let Some(info) = self.structs.get(name) else {
                    return;
                };
                for ((_, param), argument) in info
                    .origin_slots()
                    .into_iter()
                    .zip(info.origin_tail(arguments))
                {
                    let TyArg::Origin(origin) = argument else {
                        continue;
                    };
                    let mutable = match param.origin_mutability.as_ref().map(|e| &e.kind) {
                        Some(ExprKind::Bool(true)) => Some(true),
                        Some(ExprKind::Bool(false)) => Some(false),
                        _ => None,
                    };
                    self.record_origin_places(access, origin, mutable);
                }
            }
            Ty::Pointer { element, origin } => {
                self.record_carried_places(access, element);
                if let PointerOrigin::Place { place, mutable } = origin {
                    access.carry(place, *mutable);
                }
            }
            Ty::Ref(reference) => {
                self.record_carried_places(access, &reference.referent);
                self.record_origin_places(
                    access,
                    &reference.origin,
                    match reference.mutability {
                        Mutability::Mutable => Some(true),
                        Mutability::Immutable => Some(false),
                        Mutability::Param(_) => None,
                    },
                );
            }
            Ty::Tuple(elements) => {
                for element in elements {
                    self.record_carried_places(access, element);
                }
            }
            _ => {}
        }
    }

    fn record_origin_places(
        &self,
        access: &mut ArgumentAccess,
        origin: &Origin,
        mutable: Option<bool>,
    ) {
        match origin {
            Origin::Place(place) => {
                let mutable = mutable.unwrap_or_else(|| self.owner_is_mutable(place.root));
                access.carry(place, mutable);
            }
            Origin::Union(members) => {
                for member in members {
                    self.record_origin_places(access, member, mutable);
                }
            }
            Origin::Param(_)
            | Origin::SelfParam
            | Origin::Static
            | Origin::Untracked { .. }
            | Origin::Unbound => {}
        }
    }
}
