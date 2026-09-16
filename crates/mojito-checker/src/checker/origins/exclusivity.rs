//! Argument exclusivity over carried origins: a call may not hand one
//! callee a mutable path to some storage through one argument and any path
//! to the same storage through another, whether the path is the argument's
//! own place (a `mut` parameter) or an origin its declared parameter type
//! names (`RefBox[o]`, `List[RefBox[o]]`, a `Pointer[T, o]`). Current Mojo
//! rejects `stash(sink, RefBox(Pointer(to=view)))` over `mut sink:
//! List[RefBox[o]], var box: RefBox[o]` this way, while a generic
//! `append(mut self, var value: Self.T)` names no origin and is accepted.
//! Two arguments' *own* places are the syntactic place rule's business
//! (`check_call_aliasing`); this rule judges every pair with a carried origin
//! on at least one side.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::origin::{Mutability, Origin, OriginPlace, PointerOrigin, places_overlap};

/// One argument's access to caller storage: its own place when passed by
/// reference or read by borrow, and the places its declared type carries.
struct ArgumentAccess {
    name: String,
    own: Option<(OriginPlace, bool)>,
    carried: Vec<(OriginPlace, bool)>,
}

impl ArgumentAccess {
    const fn new(name: String) -> Self {
        Self {
            name,
            own: None,
            carried: Vec::new(),
        }
    }

    fn carry(&mut self, place: &OriginPlace, mutable: bool) {
        let place = place.without_subtree();
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
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn check_free_call_aliasing(
        &self,
        callee: &str,
        names: &[String],
        conventions: &[Option<ArgConvention>],
        copied_reads: &[bool],
        declared: &[Ty],
        slots: &[ArgSlot],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        crate::checker::places::reject_transfer_into_mutable(
            callee,
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
        self.check_argument_origin_exclusivity(
            callee,
            None,
            names,
            conventions,
            declared,
            slots,
            args,
            kwargs,
        )
    }

    /// Reject a call two of whose arguments (the receiver included) reach
    /// overlapping caller storage with at least one mutable path, reporting
    /// the first such pair in argument order as current Mojo does.
    ///
    /// `declared` are the callee's declared parameter types with its origin
    /// binders bound for this call and its type parameters left abstract, so
    /// an origin visible only through a type argument (`value: T` bound to a
    /// `RefBox[origin_of(view)]`) does not count, as at the pin. `receiver`
    /// is the receiver expression, its convention, and the declared `Self`
    /// type with the receiver's origin tail bound.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn check_argument_origin_exclusivity(
        &self,
        callee: &str,
        receiver: Option<(&Expr, Option<ArgConvention>, &Ty)>,
        names: &[String],
        conventions: &[Option<ArgConvention>],
        declared: &[Ty],
        slots: &[ArgSlot],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        let mut accesses: Vec<ArgumentAccess> = Vec::new();
        if let Some((object, convention, self_ty)) = receiver {
            let mut access = ArgumentAccess::new("self".to_string());
            access.own = self.own_place(object, convention);
            self.record_carried_places(&mut access, self_ty);
            accesses.push(access);
        }
        for (index, slot) in slots.iter().enumerate() {
            let argument = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let mut access = ArgumentAccess::new(
                names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("arg{index}")),
            );
            access.own = self.own_place(argument, conventions.get(index).copied().flatten());
            if let Some(parameter) = declared.get(index) {
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
                    callee: callee.to_string(),
                });
            }
        }
        Ok(())
    }

    /// An argument passed by reference (`mut`/`ref`) reaches its own place
    /// mutably; a place argument read by borrow reaches it immutably; a
    /// transferred, consumed, copied, or non-place argument reaches no
    /// place of its own.
    fn own_place(
        &self,
        argument: &Expr,
        convention: Option<ArgConvention>,
    ) -> Option<(OriginPlace, bool)> {
        if matches!(argument.kind, ExprKind::Transfer(_)) {
            return None;
        }
        let place = self.origin_place(argument).ok()?.without_subtree();
        match convention {
            Some(ArgConvention::Mut | ArgConvention::Ref) => Some((place, true)),
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
