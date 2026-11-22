//! Canonical checked representation for origins and reference permissions.
//!
//! Source origin clauses are expressions because the parser must preserve
//! syntax it cannot resolve. Past the checker boundary, origins use stable
//! binding identities and deliberately lose source names.

use std::cmp::Ordering;

use crate::types::LitOriginMut;

/// Stable identity of a value binding in one checker run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerId(pub u32);

/// An owner is one check's numbering, so no encoding carries one outside
/// [`encoding_packaged_owners`]: writing a value that holds one fails,
/// naming why.
impl serde::Serialize for OwnerId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if OWNERS_ENCODE.get() {
            return serializer.serialize_u32(self.0);
        }
        Err(serde::ser::Error::custom(format!(
            "binding owner {} is local to one check",
            self.0
        )))
    }
}

impl<'de> serde::Deserialize<'de> for OwnerId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if OWNERS_ENCODE.get() {
            return u32::deserialize(deserializer).map(Self);
        }
        Err(serde::de::Error::custom(
            "a binding owner is local to one check",
        ))
    }
}

/// Run `f` with [`OwnerId`]'s encoding permitted: inside it an owner
/// encodes and decodes as its bare number.
///
/// A package's bodies carry the package build's binding owners in their
/// types verbatim, as their MIR text spelled them (`$v<n>`); only the
/// package's body records are written and read under this permit.
pub fn encoding_packaged_owners<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            OWNERS_ENCODE.set(self.0);
        }
    }
    let _restore = Restore(OWNERS_ENCODE.replace(true));
    f()
}

/// Stable identity of a declared or inferred origin parameter.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct OriginParamId(pub u32);

impl OriginParamId {
    /// The first id of the mutability binders a callable infers for its
    /// bare `ref` parameters, above every declared binder's index.
    const REFERENCE_BINDER_BASE: u32 = 1 << 16;

    /// The `mut` binder upstream infers for the bare `ref` parameter at
    /// `slot` (the receiver counted), as `ref self` is
    /// `self: ref[origin_of(self)] Self` over an inferred
    /// `Origin[mut=self_mut]`: a reference reached through the parameter
    /// carries it, and a call binds it to its argument's mutability.
    #[must_use]
    pub const fn reference_binder(slot: u32) -> Self {
        Self(Self::REFERENCE_BINDER_BASE + slot)
    }

    /// The implicit origin upstream gives a callable's heterogeneous
    /// collector (`*args: *Ts`), above every reference binder: the
    /// collector's `VariadicPack[origin=args, ...]` type carries it, so an
    /// element reference (`args[i]`, `ref[Self.origin]`) names it
    /// (`args.origin`) rather than any binder the callable declares. A
    /// callable has at most one positional collector, so one id serves.
    #[must_use]
    pub const fn pack_binder() -> Self {
        Self(Self::PACK_BINDER_ID)
    }

    /// Whether this is [`Self::pack_binder`].
    #[must_use]
    pub const fn is_pack_binder(self) -> bool {
        self.0 == Self::PACK_BINDER_ID
    }

    const PACK_BINDER_ID: u32 = 1 << 17;
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
/// One projection step within an origin-tracked place.
pub enum OriginSeg {
    /// A named struct field.
    Field(String),
    /// An index whose value is not part of the static origin identity.
    AnyIndex,
    /// Storage owned behind a container, identified by a compile-time tag.
    ///
    /// Unlike `AnyIndex`, an interior segment denotes an invalidation domain:
    /// mutating its base starts a new generation without making ordinary reads
    /// of the base conflict with references into the old generation.
    Interior(String),
    /// The conservative subtree form (`origin._subtree`): the base place or
    /// any origin formed by field/interior projections beneath it. Always the
    /// terminal segment of a path. For overlap it is a wildcard over every
    /// descendant, and unlike `Interior` its generation is invalidated by a
    /// mutation anywhere at or below the base — including a write through the
    /// subtree reference itself, which self-invalidates after its first write.
    Subtree,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
/// Stable owner plus its statically known projection path.
pub struct OriginPlace {
    pub root: OwnerId,
    pub path: Vec<OriginSeg>,
}

impl OriginPlace {
    /// The place with its `_subtree` projections removed: the storage
    /// identity a subtree pointer refines rather than changes.
    #[must_use]
    pub fn without_subtree(&self) -> Self {
        Self {
            root: self.root,
            path: self
                .path
                .iter()
                .filter(|segment| !matches!(segment, OriginSeg::Subtree))
                .cloned()
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Canonical checked description of storage a reference may designate.
pub enum Origin {
    Param(OriginParamId),
    /// The receiver's origin in an abstract signature where no `self` place is
    /// bound (a trait method's `origin_of(self)` in an associated-type
    /// application). It is resolved to the concrete receiver origin at the
    /// conformance/call site, and — like every non-`Place` origin — erases from
    /// the runtime ABI.
    SelfParam,
    Place(OriginPlace),
    Union(Vec<Self>),
    Static,
    Untracked {
        mutable: bool,
    },
    /// A struct origin slot the use site infers and this type leaves unbound
    /// (upstream's `_`/`...` placeholder, or an omitted slot where the
    /// annotation position permits omission). Any origin coerces into it; it
    /// coerces into nothing concrete and designates no storage.
    Unbound,
    /// Upstream's `#lit.origin.mutcast<o> : !lit.origin<m>`, the origin
    /// `o.unsafe_mut_cast[m]()` builds and `ImmOrigin(o)` is (at `m =
    /// False`): the same storage as `o`, as an `Origin[mut=m]` value, so a
    /// slot's infer-only `mut` binds `m` and a `Pointer[T, Self.o]` member
    /// names it at `m`. Built only through [`Origin::mut_cast`], so never
    /// nested and never a cast to the mutability its base already has.
    MutCast(Box<Self>, LitOriginMut),
    /// Upstream's `#lit.interior.origin<…>` / `#lit.origin.subtree<…>` over
    /// an origin binder or the receiver (`o._get_owned_interior["tag"]`,
    /// `o._subtree`): `base` is `Param` or `SelfParam`, and the path is one
    /// or more `Interior` tags, then an optional terminal `Subtree`, or a
    /// lone `Subtree`. A place-rooted projection stays in
    /// [`OriginPlace::path`].
    Projected(Box<Self>, Vec<OriginSeg>),
    /// Upstream's `#lit.any.origin` (`ImmUnsafeAnyOrigin`,
    /// `MutUnsafeAnyOrigin`): a distinct origin from the untracked empty
    /// union, which like it names no storage this check tracks.
    UnsafeAny {
        mutable: bool,
    },
}

impl Origin {
    /// The origin a raw `!lit.origin` attribute names, and its mutability:
    /// `shape` is the attribute's whitespace-free spelling, its one `{}`
    /// hole the `i1` mutability `operand` holds. Upstream's named origins
    /// are these attributes: `#lit.any.origin` (`AnyOrigin`,
    /// `UnsafeAnyOrigin`), the empty `#lit.origin.union<>`
    /// (`UntrackedOrigin`), and the static constants' field of
    /// `#lit.static.origin` (`ImmStaticOrigin`). `None` for any other
    /// attribute.
    #[must_use]
    pub fn from_lit_attribute(shape: &str, operand: Option<bool>) -> Option<(Self, bool)> {
        let (head, mutable) = match operand {
            Some(mutable) => (shape.strip_suffix(":!lit.origin<{}>")?, mutable),
            None => shape
                .strip_suffix(":!lit.origin<true>")
                .map(|head| (head, true))
                .or_else(|| {
                    shape
                        .strip_suffix(":!lit.origin<false>")
                        .map(|head| (head, false))
                })?,
        };
        let origin = match head {
            "#lit.any.origin" => Self::UnsafeAny { mutable },
            "#lit.origin.union<>" => Self::Untracked { mutable },
            head if !mutable
                && head
                    == STATIC_CONSTANTS_ORIGIN
                        .split_whitespace()
                        .collect::<String>() =>
            {
                Self::Static
            }
            _ => return None,
        };
        Some((origin, mutable))
    }

    /// The raw `!lit.origin` attribute this origin is at mutability
    /// `mutable`, as [`Origin::from_lit_attribute`] reads it: `None` for an
    /// origin no named origin is.
    #[must_use]
    pub fn lit_attribute(&self, mutable: bool) -> Option<String> {
        let head = match self {
            Self::UnsafeAny { .. } => "#lit.any.origin",
            Self::Untracked { .. } => "#lit.origin.union<>",
            Self::Static => {
                return Some(format!("{STATIC_CONSTANTS_ORIGIN} : !lit.origin<false>"));
            }
            _ => return None,
        };
        Some(format!("{head} : !lit.origin<{mutable}>"))
    }

    /// Upstream's `_get_owned_interior[tag]`: the interior-generation
    /// projection `tag` below this origin, through unions and an
    /// `ImmOrigin(...)` cast. `None` for an origin naming no tracked storage
    /// (static, untracked, unsafe-any, unbound) and below a `_subtree`.
    #[must_use]
    pub fn owned_interior(&self, tag: &str) -> Option<Self> {
        self.project(&OriginSeg::Interior(tag.to_owned()))
    }

    /// Upstream's `_subtree`: the conservative subtree projection of this
    /// origin, under the same rules as [`Origin::owned_interior`].
    #[must_use]
    pub fn subtree(&self) -> Option<Self> {
        self.project(&OriginSeg::Subtree)
    }

    /// Whether this origin names no storage the check tracks: the untracked
    /// empty union or the unsafe-any origin.
    #[must_use]
    pub const fn is_untracked(&self) -> bool {
        matches!(self, Self::Untracked { .. } | Self::UnsafeAny { .. })
    }

    /// The origin binder or receiver a projected origin is rooted in, or the
    /// origin itself.
    #[must_use]
    pub fn projection_base(&self) -> &Self {
        match self {
            Self::Projected(base, _) => base,
            other => other,
        }
    }

    /// The `ImmOrigin(origin)` cast of `origin`, upstream's
    /// `origin.unsafe_mut_cast[False]()` ([`Origin::mut_cast`]).
    #[must_use]
    pub fn immutable(origin: Self) -> Self {
        Self::mut_cast(origin, LitOriginMut::Known(false))
    }

    /// Upstream's `origin.unsafe_mut_cast[mutability]()`, the
    /// `#lit.origin.mutcast` attribute, canonical as the pin compares it: a
    /// cast of a cast is the outer cast of the base, a known cast to the
    /// mutability the origin already has is the origin itself, an untracked
    /// or unsafe-any origin takes the flag, and an unbound slot stays
    /// unbound.
    #[must_use]
    pub fn mut_cast(origin: Self, mutability: LitOriginMut) -> Self {
        let origin = match origin {
            Self::MutCast(base, _) => *base,
            other => other,
        };
        match (origin, mutability) {
            (Self::Unbound, _) => Self::Unbound,
            (Self::Untracked { .. }, LitOriginMut::Known(mutable)) => Self::Untracked { mutable },
            (Self::UnsafeAny { .. }, LitOriginMut::Known(mutable)) => Self::UnsafeAny { mutable },
            (origin, LitOriginMut::Known(mutable))
                if origin.static_mutability() == Some(mutable) =>
            {
                origin
            }
            (origin, mutability) => Self::MutCast(Box::new(origin), mutability),
        }
    }

    /// The `mut` an `Origin[mut=...]` binder bound to this origin takes when
    /// the origin alone decides it: a known cast's, `false` for the static
    /// origin, and an untracked or unsafe-any origin's flag. `None` where
    /// the storage it names (or a binder) decides it.
    pub const fn static_mutability(&self) -> Option<bool> {
        match self {
            Self::MutCast(_, LitOriginMut::Known(mutable))
            | Self::Untracked { mutable }
            | Self::UnsafeAny { mutable } => Some(*mutable),
            Self::Static => Some(false),
            _ => None,
        }
    }

    /// The `mut` an `Origin[mut=mut]` binder takes when only an origin
    /// binder decides it: the mutability of the binder this origin is
    /// rooted in, through projections and mutability casts. `None` for an
    /// origin no binder roots (a place, the receiver, a fixed origin).
    #[must_use]
    pub fn binder_mutability(&self) -> Option<Mutability> {
        match self {
            Self::Param(id) => Some(Mutability::Param(*id)),
            Self::Projected(base, _) | Self::MutCast(base, _) => base.binder_mutability(),
            _ => None,
        }
    }

    /// The symbolic mutability a cast to a binder's `Bool` holds
    /// (`o.unsafe_mut_cast[m]()`); `None` for any other origin.
    #[must_use]
    pub const fn symbolic_cast_mutability(&self) -> Option<&crate::param_expr::ParamExpr> {
        match self {
            Self::MutCast(_, LitOriginMut::Expr(mutability)) => Some(mutability),
            _ => None,
        }
    }

    /// Whether a symbolic cast mutability within this origin, through
    /// unions and projections, satisfies `predicate`.
    pub fn any_cast_mutability(
        &self,
        predicate: &mut dyn FnMut(&crate::param_expr::ParamExpr) -> bool,
    ) -> bool {
        match self {
            Self::MutCast(base, mutability) => {
                matches!(mutability, LitOriginMut::Expr(read) if predicate(read))
                    || base.any_cast_mutability(predicate)
            }
            Self::Union(members) => members
                .iter()
                .any(|member| member.any_cast_mutability(predicate)),
            Self::Projected(base, _) => base.any_cast_mutability(predicate),
            _ => false,
        }
    }

    /// This origin with each symbolic cast mutability rewritten by
    /// `rewrite`, through unions and projections: a cast whose mutability
    /// folds is canonical again ([`Origin::mut_cast`]).
    pub fn try_map_cast_mutability(
        &self,
        rewrite: &mut dyn FnMut(
            &crate::param_expr::ParamExpr,
        ) -> Result<
            crate::param_expr::ParamExpr,
            crate::param_expr::ParamError,
        >,
    ) -> Result<Self, crate::param_expr::ParamError> {
        if !self.any_cast_mutability(&mut |_| true) {
            return Ok(self.clone());
        }
        Ok(match self {
            Self::MutCast(base, mutability) => Self::mut_cast(
                base.try_map_cast_mutability(rewrite)?,
                mutability.try_map(&mut *rewrite)?,
            ),
            Self::Union(members) => Self::union(
                members
                    .iter()
                    .map(|member| member.try_map_cast_mutability(rewrite))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Projected(base, path) => Self::Projected(
                Box::new(base.try_map_cast_mutability(rewrite)?),
                path.clone(),
            ),
            other => other.clone(),
        })
    }

    /// The storage this origin designates, below a mutability cast.
    #[must_use]
    pub fn uncast(&self) -> &Self {
        match self {
            Self::MutCast(inner, _) => inner,
            other => other,
        }
    }

    /// Whether this is an `ImmOrigin(...)` cast, one to `False`.
    #[must_use]
    pub const fn is_immutable_cast(&self) -> bool {
        matches!(self, Self::MutCast(_, LitOriginMut::Known(false)))
    }

    /// Whether a struct origin argument `self` satisfies the slot `expected`
    /// when a struct type coerces into another: an unbound slot accepts any
    /// origin, a struct binder or a concrete place demands identity, a
    /// `_subtree` slot also accepts the storage it projects, and an
    /// untracked or union slot accepts what lies within it.
    pub fn coerces_to(&self, expected: &Self) -> bool {
        if matches!(expected, Self::Unbound) || self == expected {
            return true;
        }
        // A mutability cast is a distinct origin value: it agrees only with
        // a cast to the same mutability over agreeing storage. The checker
        // folds a cast to the mutability its storage already has before
        // comparing.
        match (self, expected) {
            (Self::MutCast(actual, own), Self::MutCast(slot, mutability)) => {
                return own == mutability && actual.coerces_to(slot);
            }
            (_, Self::MutCast(..)) => return false,
            _ => {}
        }
        match expected {
            // A `_subtree` slot accepts the storage it projects, as upstream
            // converts `S[o]` to `S[o._subtree]`; a subtree origin fits
            // only its own slot.
            Self::Place(_) | Self::Projected(..) => {
                let actual = self.without_subtree();
                actual == *self && actual == expected.without_subtree()
            }
            Self::Unbound | Self::Param(_) | Self::SelfParam | Self::Static | Self::MutCast(..) => {
                false
            }
            Self::Untracked { .. } | Self::UnsafeAny { .. } => !matches!(self, Self::Unbound),
            Self::Union(members) => match self {
                Self::Unbound => false,
                Self::Union(actual) => actual.iter().all(|member| member.coerces_to(expected)),
                _ => members.iter().any(|member| self.coerces_to(member)),
            },
        }
    }

    /// The origin with each origin parameter `origins` binds replaced by its
    /// argument, through unions.
    #[must_use]
    pub fn substitute_params<S: std::hash::BuildHasher>(
        &self,
        origins: &std::collections::HashMap<u32, Self, S>,
    ) -> Self {
        match self {
            Self::Param(id) => origins.get(&id.0).cloned().unwrap_or_else(|| self.clone()),
            Self::Union(members) => Self::Union(
                members
                    .iter()
                    .map(|member| member.substitute_params(origins))
                    .collect(),
            ),
            Self::MutCast(inner, mutability) => {
                Self::mut_cast(inner.substitute_params(origins), mutability.clone())
            }
            Self::Projected(base, path) => base.substitute_params(origins).reprojected(path),
            _ => self.clone(),
        }
    }

    /// The origin with each trailing `_subtree` projection removed: the
    /// storage identity a subtree projection refines rather than changes.
    #[must_use]
    pub fn without_subtree(&self) -> Self {
        match self {
            Self::Place(place) => Self::Place(place.without_subtree()),
            Self::Projected(base, path) => match path.as_slice() {
                [OriginSeg::Subtree] => (**base).clone(),
                [prefix @ .., OriginSeg::Subtree] => Self::Projected(base.clone(), prefix.to_vec()),
                _ => self.clone(),
            },
            other => other.clone(),
        }
    }

    /// The origin with its trailing `_get_owned_interior` projections
    /// removed: the owner whose origin, upstream, already names the storage
    /// an interior generation stages (`List.unsafe_ptr()`'s elements are
    /// `origin_of(list)` at the pin).
    #[must_use]
    pub fn interior_owner(&self) -> Self {
        let owner_path = |path: &[OriginSeg]| {
            let kept = path
                .iter()
                .rposition(|segment| !matches!(segment, OriginSeg::Interior(_)))
                .map_or(0, |last| last + 1);
            path[..kept].to_vec()
        };
        match self {
            Self::Place(place) => Self::Place(OriginPlace {
                root: place.root,
                path: owner_path(&place.path),
            }),
            Self::Projected(base, path) => match owner_path(path) {
                kept if kept.is_empty() => (**base).clone(),
                kept => Self::Projected(base.clone(), kept),
            },
            Self::MutCast(inner, mutability) => {
                Self::mut_cast(inner.interior_owner(), mutability.clone())
            }
            Self::Union(members) => Self::union(members.iter().map(Self::interior_owner)),
            other => other.clone(),
        }
    }

    /// The origin with each of `path`'s interior and subtree segments
    /// projected in turn ([`Origin::owned_interior`], [`Origin::subtree`]).
    #[must_use]
    pub fn project_path(&self, path: &[OriginSeg]) -> Option<Self> {
        path.iter()
            .try_fold(self.clone(), |origin, segment| origin.project(segment))
    }

    /// A projection's base substituted to `self`, with the projection's
    /// `path` re-applied: an origin naming no tracked storage has no region
    /// to project, so it stands whole, as an explicit
    /// `MutUntrackedOrigin._get_owned_interior[...]` argument does.
    #[must_use]
    pub fn reprojected(self, path: &[OriginSeg]) -> Self {
        self.project_path(path).unwrap_or(self)
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Param(id) => write!(f, "origin#{}", id.0),
            Self::SelfParam => write!(f, "origin_of(self)"),
            Self::Unbound => write!(f, "_"),
            Self::Place(place) => {
                write!(f, "origin_of(#{}", place.root.0)?;
                for segment in &place.path {
                    match segment {
                        OriginSeg::Field(name) => write!(f, ".{name}")?,
                        OriginSeg::AnyIndex => write!(f, "[_]")?,
                        OriginSeg::Interior(tag) => write!(f, ".<{tag}>")?,
                        OriginSeg::Subtree => write!(f, "~")?,
                    }
                }
                write!(f, ")")
            }
            Self::Union(origins) => {
                for (index, origin) in origins.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{origin}")?;
                }
                Ok(())
            }
            Self::Static => write!(f, "ImmStaticOrigin"),
            Self::MutCast(inner, LitOriginMut::Known(false)) => write!(f, "ImmOrigin({inner})"),
            Self::MutCast(inner, LitOriginMut::Known(mutable)) => {
                write!(
                    f,
                    "{inner}.unsafe_mut_cast[{}]()",
                    if *mutable { "True" } else { "False" }
                )
            }
            Self::MutCast(inner, LitOriginMut::Expr(mutability)) => {
                write!(f, "{inner}.unsafe_mut_cast[{mutability}]()")
            }
            Self::Projected(base, path) => {
                write!(f, "{base}")?;
                for segment in path {
                    match segment {
                        OriginSeg::Interior(tag) => write!(f, "._get_owned_interior[\"{tag}\"]")?,
                        OriginSeg::Subtree => write!(f, "._subtree")?,
                        OriginSeg::Field(name) => write!(f, ".{name}")?,
                        OriginSeg::AnyIndex => write!(f, "[_]")?,
                    }
                }
                Ok(())
            }
            Self::UnsafeAny { mutable } => {
                write!(f, "{}UnsafeAnyOrigin", if *mutable { "Mut" } else { "Imm" })
            }
            Self::Untracked { mutable } => {
                write!(f, "{}UntrackedOrigin", if *mutable { "Mut" } else { "Imm" })
            }
        }
    }
}

/// Access performed through one captured origin when a callable is invoked.
///
/// This is deliberately part of the static environment summary: an `OriginSet`
/// by itself extends lifetimes but cannot tell persistent-loan analysis whether
/// an indirect call reads or may mutate captured storage.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum CaptureAccess {
    Read,
    Write,
}

/// One canonical concrete dependency in a capturing callable environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CaptureOrigin {
    pub origin: Origin,
    pub access: CaptureAccess,
}

impl CaptureOrigin {
    pub const fn read(origin: Origin) -> Self {
        Self {
            origin,
            access: CaptureAccess::Read,
        }
    }

    pub const fn write(origin: Origin) -> Self {
        Self {
            origin,
            access: CaptureAccess::Write,
        }
    }
}

/// The origin dependencies of a capturing callable.
///
/// Mojo's `OriginSet` is deliberately separate from an [`Origin`] union: it is
/// the canonical set of owner origins retained by a callable environment, not
/// an origin a reference value can designate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CaptureOriginSet(pub Vec<CaptureOrigin>);

impl CaptureOriginSet {
    /// Construct a concrete set with nested unions flattened, duplicates
    /// removed, and members in the same canonical order used by [`Origin`]. If
    /// the same exact origin is both read and written, the write summary
    /// subsumes the read.
    pub fn concrete(captures: impl IntoIterator<Item = CaptureOrigin>) -> Self {
        let mut members = Vec::new();
        for capture in captures {
            match capture.origin {
                Origin::Union(origins) => {
                    members.extend(origins.into_iter().map(|origin| CaptureOrigin {
                        origin,
                        access: capture.access,
                    }));
                }
                origin => members.push(CaptureOrigin {
                    origin,
                    access: capture.access,
                }),
            }
        }
        members.sort_by(|left, right| {
            cmp_origin(&left.origin, &right.origin).then(left.access.cmp(&right.access))
        });
        let mut canonical: Vec<CaptureOrigin> = Vec::with_capacity(members.len());
        for member in members {
            if let Some(previous) = canonical.last_mut()
                && previous.origin == member.origin
            {
                previous.access = previous.access.max(member.access);
            } else {
                canonical.push(member);
            }
        }
        Self(canonical)
    }

    pub const fn empty() -> Self {
        Self(Vec::new())
    }
}

/// How a checked callable relates to an environment.
///
/// `Default` is the unqualified `def(...)` callable constraint. It is not an
/// implicit wildcard coercion: later bound solving may explicitly accept a
/// concrete environment for such a constraint, but ordinary value coercion
/// compares this fact exactly. `Thin` is an explicitly non-capturing function;
/// `Capturing` retains its `OriginSet` contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum CallableEnvironment {
    #[default]
    Default,
    Thin,
    Capturing(CaptureOriginSet),
}

impl CallableEnvironment {
    /// The capture set the environment retains when invoked.
    pub const fn captures(&self) -> Option<&CaptureOriginSet> {
        match self {
            Self::Capturing(captures) => Some(captures),
            Self::Default | Self::Thin => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Permission available through a checked reference.
pub enum Mutability {
    Immutable,
    Mutable,
    Param(OriginParamId),
}

impl Mutability {
    /// The receiver's symbolic mutability, which its origin
    /// (`origin_of(self)`) carries throughout a method, signature and body
    /// alike; a call site rebases it onto the concrete receiver.
    pub const RECEIVER: Self = Self::Param(OriginParamId(0));

    /// The closed mutability `mutable` says.
    #[must_use]
    pub const fn of(mutable: bool) -> Self {
        if mutable {
            Self::Mutable
        } else {
            Self::Immutable
        }
    }

    /// This mutability as a `Pointer`'s `mut` argument: a `Bool`, or the
    /// implicit `mut` value binder of the origin binder that decides it
    /// (upstream's `o: Origin[mut=m]` ties a pointer's `mut` to `m`).
    #[must_use]
    pub fn mut_argument(self) -> crate::ct::CtValue {
        match self {
            Self::Mutable => crate::ct::CtValue::Bool(true),
            Self::Immutable => crate::ct::CtValue::Bool(false),
            Self::Param(id) => crate::ct::CtValue::Deferred(crate::param_expr::ParamRef {
                id: crate::param_expr::ParamId::new(ORIGIN_MUT_BINDER_OWNER, id.0 as usize),
                name: std::sync::Arc::from("mut"),
            }),
        }
    }

    /// The origin binder's mutability a `Pointer`'s deferred `mut`
    /// argument names ([`Mutability::mut_argument`]), or `None` for any
    /// other binder.
    #[must_use]
    pub fn of_mut_argument(binder: &crate::param_expr::ParamRef) -> Option<Self> {
        (&*binder.id.owner == ORIGIN_MUT_BINDER_OWNER)
            .then(|| u32::try_from(binder.id.slot).ok())
            .flatten()
            .map(|slot| Self::Param(OriginParamId(slot)))
    }
}

/// The pointer projection a clone origin binder re-applies for a caller place.
///
/// When the binder stands for a caller place
/// ([`PointerOrigin::clone_bindable_place`]), it carries the place's
/// mutability, then the interior-generation tags and terminal `._subtree`
/// below the base place the binder binds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CloneBinderProjection {
    pub mutable: bool,
    pub interior: Vec<String>,
    pub subtree: bool,
}

impl CloneBinderProjection {
    /// The binder pointer's provenance: `id` at this mutability and
    /// projection.
    pub fn binder_origin(&self, id: OriginParamId) -> PointerOrigin {
        PointerOrigin::Param {
            id,
            mutability: if self.mutable {
                Mutability::Mutable
            } else {
                Mutability::Immutable
            },
            interior: self.interior.clone(),
            subtree: self.subtree,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PointerOrigin {
    Place {
        place: OriginPlace,
        mutable: bool,
    },
    Param {
        id: OriginParamId,
        mutability: Mutability,
        /// Interior-generation tags projected below the parameter's place
        /// (`origin._get_owned_interior["tag"]`). A non-empty path marks a
        /// multi-element domain: substitution appends it to the solved place,
        /// so the concrete origin ends in `OriginSeg::Interior` and offset
        /// dereferences stay legal after monomorphized replay.
        interior: Vec<String>,
        /// A trailing `._subtree` projection: substitution appends a terminal
        /// `OriginSeg::Subtree` after the interior tags. A subtree pointer is
        /// a conservative single reference, never a multi-element domain.
        subtree: bool,
    },
    /// `origin_of(self)` (optionally interior-projected) in a method
    /// signature or body: the receiver's own place, symbolic until a call
    /// site rebases it onto the concrete receiver. Both the declared return
    /// annotation and the body's `unsafe_origin_cast` target resolve to this same
    /// form, so return coercion compares equal without a bound `self` place.
    SelfPlace {
        mutability: Mutability,
        interior: Vec<String>,
        /// A trailing `._subtree` projection (see [`PointerOrigin::Param`]).
        subtree: bool,
    },
    /// `ImmStaticOrigin`, and at `mutable` its
    /// `ImmStaticOrigin.unsafe_mut_cast[True]()` cast: a string literal's
    /// static storage, which holds no loan.
    Static {
        mutable: bool,
    },
    Untracked {
        mutable: bool,
    },
    /// A placeholder-origin pointer (`ImmPointer[T, _]`, `MutPointer[T, _]`,
    /// and the bare `Pointer[T, _]` parameter, which reads as the immutable
    /// alias): any provenance binds and no loan is held; only the permission
    /// is retained.
    UnsafeAny {
        mutable: bool,
    },
    /// `origin_of(a, b)`: the normalized [`Origin::union`] of at least two
    /// concrete members (places, and the static, untracked, and unsafe-any
    /// origins a union keeps). This is the view of a pointer's
    /// `Origin::Union` argument that the pointer accessors and a `!lit.ref`
    /// give.
    Union {
        members: Vec<Origin>,
        mutability: Mutability,
    },
}

impl PointerOrigin {
    /// The clone origin binder a provenance naming a caller place can be
    /// baked to: a place reached through fields, then any interior-generation
    /// tags and an optional terminal `._subtree`, which the binder's pointer
    /// re-applies below the base place it stands for. A place with a field
    /// below an interior segment has no such binder. Such a place never keys
    /// a clone's identity.
    pub fn clone_bindable_place(&self) -> Option<CloneBinderProjection> {
        let Self::Place { place, mutable } = self else {
            return None;
        };
        let subtree = matches!(place.path.last(), Some(OriginSeg::Subtree));
        let projected = &place.path[..place.path.len() - usize::from(subtree)];
        let base = projected
            .iter()
            .take_while(|segment| matches!(segment, OriginSeg::Field(_) | OriginSeg::AnyIndex))
            .count();
        let interior = projected[base..]
            .iter()
            .map(|segment| match segment {
                OriginSeg::Interior(tag) => Some(tag.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some(CloneBinderProjection {
            mutable: *mutable,
            interior,
            subtree,
        })
    }

    /// The provenance a binder pointer projected by `interior` and
    /// `subtree` binds its binder to when it matches `self`: `self` with
    /// that trailing projection removed, or `None` when `self` does not end
    /// in it.
    pub fn without_projection(&self, interior: &[String], subtree: bool) -> Option<Self> {
        match self {
            Self::Place { place, mutable } => {
                let tail = interior
                    .iter()
                    .map(|tag| OriginSeg::Interior(tag.clone()))
                    .chain(subtree.then_some(OriginSeg::Subtree))
                    .collect::<Vec<_>>();
                place.path.ends_with(&tail).then(|| Self::Place {
                    place: OriginPlace {
                        root: place.root,
                        path: place.path[..place.path.len() - tail.len()].to_vec(),
                    },
                    mutable: *mutable,
                })
            }
            Self::Param {
                id,
                mutability,
                interior: own,
                subtree: own_subtree,
            } if *own_subtree == subtree && own.ends_with(interior) => Some(Self::Param {
                id: *id,
                mutability: *mutability,
                interior: own[..own.len() - interior.len()].to_vec(),
                subtree: false,
            }),
            _ => (interior.is_empty() && !subtree).then(|| self.clone()),
        }
    }

    /// This provenance as upstream's `Pointer[mut, T, origin]` spells it: the
    /// `origin` struct argument and the `mut` argument. Lossless:
    /// [`PointerOrigin::from_parts`] inverts it.
    pub fn parts(&self) -> (Origin, Mutability) {
        self.clone().into_parts()
    }

    /// [`PointerOrigin::parts`], consuming the provenance.
    pub fn into_parts(self) -> (Origin, Mutability) {
        let projected = |base: Origin, interior: &[String], subtree: bool| {
            let path: Vec<_> = interior
                .iter()
                .map(|tag| OriginSeg::Interior(tag.clone()))
                .chain(subtree.then_some(OriginSeg::Subtree))
                .collect();
            if path.is_empty() {
                base
            } else {
                Origin::Projected(Box::new(base), path)
            }
        };
        match self {
            Self::Place { place, mutable } => (Origin::Place(place), Mutability::of(mutable)),
            Self::Param {
                id,
                mutability,
                interior,
                subtree,
            } => (projected(Origin::Param(id), &interior, subtree), mutability),
            Self::SelfPlace {
                mutability,
                interior,
                subtree,
            } => (projected(Origin::SelfParam, &interior, subtree), mutability),
            Self::Static { mutable } => (
                Origin::mut_cast(Origin::Static, LitOriginMut::Known(mutable)),
                Mutability::of(mutable),
            ),
            Self::Untracked { mutable } => (Origin::Untracked { mutable }, Mutability::of(mutable)),
            Self::UnsafeAny { mutable } => (Origin::UnsafeAny { mutable }, Mutability::of(mutable)),
            Self::Union {
                members,
                mutability,
            } => (Origin::Union(members), mutability),
        }
    }

    /// The provenance a `Pointer` whose `origin` argument is `origin` and
    /// whose `mut` argument is `mutability` carries: an `ImmOrigin(...)`
    /// cast reads as the immutable pointer to its storage. `None` for an
    /// origin no pointer holds (an unbound slot, a malformed projection, a
    /// union with a binder or receiver member) and for a mutability its
    /// origin contradicts.
    pub fn from_parts(origin: Origin, mutability: Mutability) -> Option<Self> {
        let flag = |mutable: bool| match mutability {
            Mutability::Mutable => mutable,
            Mutability::Immutable => !mutable,
            Mutability::Param(_) => false,
        };
        match origin {
            Origin::MutCast(inner, LitOriginMut::Known(mutable)) if *inner == Origin::Static => {
                Some(Self::Static { mutable })
            }
            Origin::MutCast(inner, LitOriginMut::Known(mutable)) => {
                Self::from_parts(*inner, Mutability::of(mutable))
            }
            Origin::MutCast(inner, LitOriginMut::Expr(_)) => Self::from_parts(*inner, mutability),
            Origin::Place(place) => match mutability {
                Mutability::Mutable => Some(Self::Place {
                    place,
                    mutable: true,
                }),
                Mutability::Immutable => Some(Self::Place {
                    place,
                    mutable: false,
                }),
                Mutability::Param(_) => None,
            },
            Origin::Param(id) => Some(Self::Param {
                id,
                mutability,
                interior: Vec::new(),
                subtree: false,
            }),
            Origin::SelfParam => Some(Self::SelfPlace {
                mutability,
                interior: Vec::new(),
                subtree: false,
            }),
            Origin::Projected(base, path) => {
                let subtree = matches!(path.last(), Some(OriginSeg::Subtree));
                let interior = path[..path.len() - usize::from(subtree)]
                    .iter()
                    .map(|segment| match segment {
                        OriginSeg::Interior(tag) => Some(tag.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                if interior.is_empty() && !subtree {
                    return None;
                }
                match *base {
                    Origin::Param(id) => Some(Self::Param {
                        id,
                        mutability,
                        interior,
                        subtree,
                    }),
                    Origin::SelfParam => Some(Self::SelfPlace {
                        mutability,
                        interior,
                        subtree,
                    }),
                    _ => None,
                }
            }
            Origin::Static => {
                (mutability == Mutability::Immutable).then_some(Self::Static { mutable: false })
            }
            Origin::Untracked { mutable } => flag(mutable).then_some(Self::Untracked { mutable }),
            Origin::UnsafeAny { mutable } => flag(mutable).then_some(Self::UnsafeAny { mutable }),
            Origin::Union(members) => {
                members
                    .iter()
                    .all(concrete_union_member)
                    .then_some(Self::Union {
                        members,
                        mutability,
                    })
            }
            Origin::Unbound => None,
        }
    }

    /// The loan-tracked [`Origin`] a pointer provenance corresponds to, when it
    /// designates checked storage: the binder or receiver a projection is
    /// rooted in, with the projection dropped, which is the storage a loan
    /// keeps. `Static`, `Untracked`, and `UnsafeAny` pointers carry no owner
    /// loan. The struct argument a provenance spells is [`PointerOrigin::parts`].
    pub fn as_origin(&self) -> Option<Origin> {
        match self {
            Self::Place { place, .. } => Some(Origin::Place(place.clone())),
            Self::Union { members, .. } => Some(Origin::Union(members.clone())),
            Self::Param { id, .. } => Some(Origin::Param(*id)),
            Self::SelfPlace { .. } => Some(Origin::SelfParam),
            Self::Static { .. } | Self::Untracked { .. } | Self::UnsafeAny { .. } => None,
        }
    }

    /// Whether this provenance statically designates one checked place: a
    /// place, or the receiver's `origin_of(self)`, not projected into a
    /// multi-element domain. Such a pointer is a frame/slot handle, so
    /// indexing it is the offset-0 dereference and address arithmetic over
    /// it is rejected (R2212). A callable's own origin binder is not one: a
    /// call may bind it to heap storage, as a static, untracked, or
    /// unsafe-any provenance may be.
    pub fn single_place(&self) -> bool {
        matches!(self, Self::Place { .. } | Self::SelfPlace { .. }) && !self.multi_element()
    }

    /// Whether this tracked provenance designates a multi-element domain,
    /// making non-zero offset dereferences legal: a place, origin binder, or
    /// receiver origin projected into an interior-generation segment
    /// (collection-owned element storage). Untracked/static provenances
    /// never reach the offset rule.
    pub fn multi_element(&self) -> bool {
        match self {
            Self::Union { .. } => false,
            Self::Place { place, .. } => {
                matches!(place.path.last(), Some(OriginSeg::Interior(_)))
            }
            Self::Param {
                interior, subtree, ..
            }
            | Self::SelfPlace {
                interior, subtree, ..
            } => !interior.is_empty() && !subtree,
            Self::Static { .. } | Self::Untracked { .. } | Self::UnsafeAny { .. } => false,
        }
    }

    /// Whether this provenance ends in the conservative `._subtree` projection.
    /// A subtree pointer designates its root or any descendant, so nothing may
    /// re-derive an exact place from its origin: dereferences go through the
    /// runtime handle, and its generation stales on any mutation at or below
    /// the root.
    pub fn subtree(&self) -> bool {
        match self {
            Self::Union { .. } => false,
            Self::Place { place, .. } => {
                matches!(place.path.last(), Some(OriginSeg::Subtree))
            }
            Self::Param { subtree, .. } | Self::SelfPlace { subtree, .. } => *subtree,
            Self::Static { .. } | Self::Untracked { .. } | Self::UnsafeAny { .. } => false,
        }
    }

    /// Whether writes through the pointer are permitted, when that is statically
    /// known. A symbolic parameter mutability returns `None`: storage coercion
    /// rejects an immutable place only for a field whose declared mutability
    /// is explicitly `mut=True`.
    pub const fn statically_mutable(&self) -> Option<bool> {
        match self {
            Self::Place { mutable, .. }
            | Self::Untracked { mutable }
            | Self::UnsafeAny { mutable }
            | Self::Static { mutable } => Some(*mutable),
            Self::Param { mutability, .. }
            | Self::SelfPlace { mutability, .. }
            | Self::Union { mutability, .. } => match mutability {
                Mutability::Mutable => Some(true),
                Mutability::Immutable => Some(false),
                Mutability::Param(_) => None,
            },
        }
    }

    /// The same provenance with writes through the pointer forbidden.
    #[must_use]
    pub fn immutable(self) -> Self {
        match self {
            Self::Union { members, .. } => Self::Union {
                members,
                mutability: Mutability::Immutable,
            },
            Self::Place { place, .. } => Self::Place {
                place,
                mutable: false,
            },
            Self::Untracked { .. } => Self::Untracked { mutable: false },
            Self::UnsafeAny { .. } => Self::UnsafeAny { mutable: false },
            Self::Static { .. } => Self::Static { mutable: false },
            Self::Param {
                id,
                interior,
                subtree,
                ..
            } => Self::Param {
                id,
                mutability: Mutability::Immutable,
                interior,
                subtree,
            },
            Self::SelfPlace {
                interior, subtree, ..
            } => Self::SelfPlace {
                mutability: Mutability::Immutable,
                interior,
                subtree,
            },
        }
    }

    /// The same provenance with writes through the pointer permitted
    /// (upstream's `unsafe_mut_cast[True]`).
    #[must_use]
    pub fn mutable(self) -> Self {
        match self {
            Self::Union { members, .. } => Self::Union {
                members,
                mutability: Mutability::Mutable,
            },
            Self::Place { place, .. } => Self::Place {
                place,
                mutable: true,
            },
            Self::Untracked { .. } => Self::Untracked { mutable: true },
            Self::Static { .. } => Self::Static { mutable: true },
            Self::UnsafeAny { .. } => Self::UnsafeAny { mutable: true },
            Self::Param {
                id,
                interior,
                subtree,
                ..
            } => Self::Param {
                id,
                mutability: Mutability::Mutable,
                interior,
                subtree,
            },
            Self::SelfPlace {
                interior, subtree, ..
            } => Self::SelfPlace {
                mutability: Mutability::Mutable,
                interior,
                subtree,
            },
        }
    }

    /// Whether a pointer of this provenance may bind a parameter declared
    /// with `expected`: an unsafe-any expectation (`ImmPointer[T, _]`, the
    /// placeholder origin of a free function's raw-pointer parameter) accepts
    /// any provenance whose permission is at least the expected one, an
    /// immutable untracked expectation accepts the mutable untracked
    /// provenance (dropping capability), an immutable tracked expectation
    /// accepts the same provenance at any capability (upstream's `@implicit`
    /// `Pointer.__init__(other: Pointer) -> Pointer[T, ImmOrigin(other.origin)]`),
    /// and every other expectation, a mutable one included, binds exactly.
    pub fn coerces_to(&self, expected: &Self) -> bool {
        match expected {
            Self::UnsafeAny { mutable: false } => true,
            Self::UnsafeAny { mutable: true } => self.statically_mutable() == Some(true),
            Self::Untracked { mutable: false } => {
                matches!(self, Self::Untracked { .. })
            }
            Self::Place {
                place,
                mutable: false,
            } => matches!(self, Self::Place { place: actual, .. } if actual == place),
            Self::Param {
                id,
                mutability: Mutability::Immutable,
                interior,
                subtree,
            } => matches!(
                self,
                Self::Param {
                    id: actual,
                    interior: actual_interior,
                    subtree: actual_subtree,
                    ..
                } if actual == id && actual_interior == interior && actual_subtree == subtree
            ),
            Self::SelfPlace {
                mutability: Mutability::Immutable,
                interior,
                subtree,
            } => matches!(
                self,
                Self::SelfPlace {
                    interior: actual_interior,
                    subtree: actual_subtree,
                    ..
                } if actual_interior == interior && actual_subtree == subtree
            ),
            Self::Union {
                members,
                mutability,
            } => {
                let (origin, own) = self.parts();
                let origin = match origin {
                    Origin::MutCast(inner, _) => *inner,
                    origin => origin,
                };
                origin.coerces_to(&Origin::Union(members.clone()))
                    && match mutability {
                        Mutability::Immutable => true,
                        Mutability::Mutable => self.statically_mutable() == Some(true),
                        Mutability::Param(_) => own == *mutability,
                    }
            }
            _ => self == expected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Checked reference type `!lit.ref<referent, origin, address_space>`:
/// referent, storage origin, access permission, and the address space its
/// storage lives in.
pub struct RefTy {
    pub referent: crate::types::Ty,
    pub origin: Origin,
    pub mutability: Mutability,
    pub address_space: crate::types::AddressSpaceSlot,
}

/// An origin in a callable contract. Unlike [`Origin`], roots name parameter
/// slots rather than checker-local bindings, so the contract survives overload
/// storage and can be substituted with caller places.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SigOrigin {
    Self_,
    Param(usize),
    /// A concrete caller-side origin captured by an explicitly specialized
    /// function value. Unlike `Param`, this contract is no longer relative to
    /// the eventual call's argument slots.
    Bound(Origin),
    Static,
    /// `ImmUntrackedOrigin` / `MutUntrackedOrigin`: an exact origin that only
    /// an untracked actual (an untracked pointer's dereference) satisfies.
    Untracked {
        mutable: bool,
    },
    /// `ImmUnsafeAnyOrigin` / `MutUnsafeAnyOrigin`: any actual origin binds;
    /// only the permission is retained.
    UnsafeAny {
        mutable: bool,
    },
    Projected(Box<Self>, Vec<OriginSeg>),
    Union(Vec<Self>),
    Infer,
    /// Every origin a value of this type names once the callable's binders
    /// are bound: the source of a transfer effect whose body stores a value
    /// of a type over those binders. It is never a reference contract's
    /// origin.
    Carried(crate::types::Ty),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Mutability component of a callable reference contract.
pub enum SigMutability {
    Immutable,
    Mutable,
    /// Position of the compile-time `Bool` binder controlling an Origin's
    /// mutability. A binder index, rather than its source spelling, makes
    /// callable contracts alpha-equivalent across independently named
    /// declarations.
    BoolParam(usize),
    Infer,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Reference result or parameter contract retained in a callable signature:
/// the origins and the one address space a `ref[...]` clause names
/// (`AddressSpace.GENERIC` when it names none).
pub struct RefSig {
    pub origin: SigOrigin,
    pub mutability: SigMutability,
    pub address_space: crate::types::AddressSpaceSlot,
}

impl Origin {
    /// Construct a canonical union: flatten nesting, remove duplicates, and
    /// sort members independently of source order.
    ///
    /// # Panics
    ///
    /// Panics if the deduplicated member set is empty after the arity match
    /// selected the single-member arm.
    pub fn union(origins: impl IntoIterator<Item = Self>) -> Self {
        let mut members = Vec::new();
        for origin in origins {
            match origin {
                Self::Union(inner) => members.extend(inner),
                other => members.push(other),
            }
        }
        members.sort_by(cmp_origin);
        members.dedup();
        match members.len() {
            0 => Self::Union(Vec::new()),
            1 => members.pop().expect("one union member"),
            _ => Self::Union(members),
        }
    }

    /// Whether two origins may designate overlapping storage.
    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MutCast(left, _), right) => left.overlaps(right),
            (left, Self::MutCast(right, _)) => left.overlaps(right),
            (Self::Union(left), right) => left.iter().any(|item| item.overlaps(right)),
            (left, Self::Union(right)) => right.iter().any(|item| left.overlaps(item)),
            (Self::Place(left), Self::Place(right)) => places_overlap(left, right),
            (Self::Projected(left, _), right) => left.overlaps(right),
            (left, Self::Projected(right, _)) => left.overlaps(right),
            (Self::Param(left), Self::Param(right)) => left == right,
            (Self::SelfParam, Self::SelfParam) | (Self::Static, Self::Static) => true,
            // Untracked origins intentionally forfeit disjointness information.
            (left, right) if left.is_untracked() || right.is_untracked() => true,
            _ => false,
        }
    }
}

impl SigOrigin {
    /// Construct a canonical signature-origin union. Callable contracts are
    /// compared structurally, so union order and duplicate spelling must not
    /// make otherwise identical contracts differ. Canonicalization recurses
    /// through projected bases because those are the common nontrivial union
    /// members in reference-return signatures.
    ///
    /// # Panics
    ///
    /// Panics if the deduplicated member set is empty after the arity match
    /// selected the single-member arm.
    pub fn union(origins: impl IntoIterator<Item = Self>) -> Self {
        let mut members = Vec::new();
        for origin in origins {
            match canonical_sig_origin(origin) {
                Self::Union(inner) => members.extend(inner),
                other => members.push(other),
            }
        }
        members.sort_by(cmp_sig_origin);
        members.dedup();
        match members.len() {
            0 => Self::Union(Vec::new()),
            1 => members.pop().expect("one signature-origin union member"),
            _ => Self::Union(members),
        }
    }
}

/// Whether two projected owner places may designate overlapping storage.
pub fn places_overlap(left: &OriginPlace, right: &OriginPlace) -> bool {
    if left.root != right.root {
        return false;
    }
    left.path.iter().zip(&right.path).all(|(a, b)| {
        a == b
            || matches!(a, OriginSeg::AnyIndex | OriginSeg::Subtree)
            || matches!(b, OriginSeg::AnyIndex | OriginSeg::Subtree)
    })
}

impl Origin {
    /// One interior or subtree projection step below this origin: a place
    /// extends its path, an origin binder or the receiver becomes
    /// [`Origin::Projected`], and nothing projects below a `_subtree` or an
    /// origin naming no tracked storage.
    fn project(&self, segment: &OriginSeg) -> Option<Self> {
        let below = |path: &[OriginSeg]| {
            (!matches!(path.last(), Some(OriginSeg::Subtree)))
                .then(|| path.iter().cloned().chain([segment.clone()]).collect())
        };
        match self {
            Self::Place(place) => Some(Self::Place(OriginPlace {
                root: place.root,
                path: below(&place.path)?,
            })),
            Self::Param(_) | Self::SelfParam => Some(Self::Projected(
                Box::new(self.clone()),
                vec![segment.clone()],
            )),
            Self::Projected(base, path) => Some(Self::Projected(base.clone(), below(path)?)),
            Self::Union(members) => members
                .iter()
                .map(|member| member.project(segment))
                .collect::<Option<Vec<_>>>()
                .map(Self::union),
            Self::MutCast(inner, mutability) => inner
                .project(segment)
                .map(|origin| Self::mut_cast(origin, mutability.clone())),
            Self::Static | Self::Untracked { .. } | Self::UnsafeAny { .. } | Self::Unbound => None,
        }
    }
}

/// The reserved owner of an origin binder's implicit `mut` value binder,
/// its slot the origin binder's id.
const ORIGIN_MUT_BINDER_OWNER: &str = "$origin_mut";

/// Upstream's `ImmStaticOrigin` attribute, the static constants' field of
/// the static origin.
const STATIC_CONSTANTS_ORIGIN: &str =
    "#lit.origin.field<#lit.static.origin : !lit.origin<false>, \"__constants__\">";

/// Whether a union member designates storage no call site substitutes: a
/// place, or a static, untracked, or unsafe-any origin, under any cast.
fn concrete_union_member(origin: &Origin) -> bool {
    matches!(
        origin.uncast(),
        Origin::Place(_) | Origin::Static | Origin::Untracked { .. } | Origin::UnsafeAny { .. }
    )
}

fn cmp_origin(left: &Origin, right: &Origin) -> Ordering {
    const fn tag(origin: &Origin) -> u8 {
        match origin {
            Origin::Param(_) => 0,
            Origin::Place(_) => 1,
            Origin::Static => 2,
            Origin::Untracked { .. } => 3,
            Origin::Union(_) => 4,
            Origin::SelfParam => 5,
            Origin::Unbound => 6,
            Origin::MutCast(..) => 7,
            Origin::Projected(..) => 8,
            Origin::UnsafeAny { .. } => 9,
        }
    }
    tag(left)
        .cmp(&tag(right))
        .then_with(|| match (left, right) {
            (Origin::Param(a), Origin::Param(b)) => a.cmp(b),
            (Origin::Place(a), Origin::Place(b)) => a.cmp(b),
            (Origin::Untracked { mutable: a }, Origin::Untracked { mutable: b })
            | (Origin::UnsafeAny { mutable: a }, Origin::UnsafeAny { mutable: b }) => a.cmp(b),
            (Origin::Projected(a, a_path), Origin::Projected(b, b_path)) => {
                cmp_origin(a, b).then_with(|| a_path.cmp(b_path))
            }
            (Origin::MutCast(a, a_mut), Origin::MutCast(b, b_mut)) => {
                cmp_origin(a, b).then_with(|| cmp_lit_origin_mut(a_mut, b_mut))
            }
            (Origin::Union(a), Origin::Union(b)) => a.len().cmp(&b.len()).then_with(|| {
                a.iter()
                    .zip(b)
                    .map(|(x, y)| cmp_origin(x, y))
                    .find(|ordering| !ordering.is_eq())
                    .unwrap_or(Ordering::Equal)
            }),
            _ => Ordering::Equal,
        })
}

fn cmp_lit_origin_mut(left: &LitOriginMut, right: &LitOriginMut) -> Ordering {
    match (left, right) {
        (LitOriginMut::Known(a), LitOriginMut::Known(b)) => a.cmp(b),
        (LitOriginMut::Known(_), LitOriginMut::Expr(_)) => Ordering::Less,
        (LitOriginMut::Expr(_), LitOriginMut::Known(_)) => Ordering::Greater,
        (LitOriginMut::Expr(a), LitOriginMut::Expr(b)) => a.to_string().cmp(&b.to_string()),
    }
}

fn canonical_sig_origin(origin: SigOrigin) -> SigOrigin {
    match origin {
        SigOrigin::Projected(base, path) => {
            SigOrigin::Projected(Box::new(canonical_sig_origin(*base)), path)
        }
        SigOrigin::Union(members) => SigOrigin::union(members),
        other => other,
    }
}

fn cmp_sig_origin(left: &SigOrigin, right: &SigOrigin) -> Ordering {
    const fn tag(origin: &SigOrigin) -> u8 {
        match origin {
            SigOrigin::Self_ => 0,
            SigOrigin::Param(_) => 1,
            SigOrigin::Bound(_) => 2,
            SigOrigin::Static => 3,
            SigOrigin::Untracked { .. } => 4,
            SigOrigin::Projected(_, _) => 5,
            SigOrigin::Infer => 6,
            SigOrigin::Union(_) => 7,
            SigOrigin::UnsafeAny { .. } => 8,
            SigOrigin::Carried(_) => 9,
        }
    }

    tag(left)
        .cmp(&tag(right))
        .then_with(|| match (left, right) {
            (SigOrigin::Param(a), SigOrigin::Param(b)) => a.cmp(b),
            (SigOrigin::Bound(a), SigOrigin::Bound(b)) => cmp_origin(a, b),
            (SigOrigin::Untracked { mutable: a }, SigOrigin::Untracked { mutable: b })
            | (SigOrigin::UnsafeAny { mutable: a }, SigOrigin::UnsafeAny { mutable: b }) => {
                a.cmp(b)
            }
            (SigOrigin::Projected(a_base, a_path), SigOrigin::Projected(b_base, b_path)) => {
                cmp_sig_origin(a_base, b_base).then_with(|| a_path.cmp(b_path))
            }
            (SigOrigin::Union(a), SigOrigin::Union(b)) => a.len().cmp(&b.len()).then_with(|| {
                a.iter()
                    .zip(b)
                    .map(|(x, y)| cmp_sig_origin(x, y))
                    .find(|ordering| !ordering.is_eq())
                    .unwrap_or(Ordering::Equal)
            }),
            (SigOrigin::Carried(a), SigOrigin::Carried(b)) => a.to_string().cmp(&b.to_string()),
            _ => Ordering::Equal,
        })
}

thread_local! {
    static OWNERS_ENCODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(root: u32, path: &[OriginSeg]) -> Origin {
        Origin::Place(OriginPlace {
            root: OwnerId(root),
            path: path.to_vec(),
        })
    }

    /// `unsafe_mut_cast` is canonical as the pin compares casts: the outer
    /// cast of a cast wins, a cast to the origin's own mutability is the
    /// origin, and an untracked origin takes the flag.
    #[test]
    fn mut_cast_canonicalizes() {
        let known = LitOriginMut::Known;
        let x = place(1, &[]);
        let imm = Origin::mut_cast(x.clone(), known(false));
        assert_eq!(imm, Origin::immutable(x.clone()));
        assert!(imm.is_immutable_cast());
        let flipped = Origin::mut_cast(imm.clone(), known(true));
        assert_eq!(flipped, Origin::MutCast(Box::new(x.clone()), known(true)));
        assert_eq!(flipped.static_mutability(), Some(true));
        assert_eq!(Origin::mut_cast(imm.clone(), known(false)), imm);
        assert!(!flipped.coerces_to(&imm) && !imm.coerces_to(&flipped));
        assert_eq!(
            Origin::mut_cast(Origin::Static, known(false)),
            Origin::Static
        );
        assert_eq!(
            Origin::mut_cast(Origin::Untracked { mutable: true }, known(false)),
            Origin::Untracked { mutable: false }
        );
        assert_eq!(
            Origin::mut_cast(Origin::Unbound, known(true)),
            Origin::Unbound
        );
        assert_eq!(flipped.to_string(), format!("{x}.unsafe_mut_cast[True]()"));
    }

    #[test]
    fn named_origin_attributes_round_trip() {
        // Upstream's named origins are these raw attributes: the
        // mutability is the operand a `{}` hole holds, or the spelled one.
        for (origin, mutable) in [
            (Origin::UnsafeAny { mutable: true }, true),
            (Origin::Untracked { mutable: false }, false),
            (Origin::Static, false),
        ] {
            let attribute = origin.lit_attribute(mutable).expect("a named origin");
            let shape: String = attribute.split_whitespace().collect();
            assert_eq!(
                Origin::from_lit_attribute(&shape, None),
                Some((origin.clone(), mutable))
            );
        }
        assert_eq!(
            Origin::from_lit_attribute("#lit.origin.union<>:!lit.origin<{}>", Some(true)),
            Some((Origin::Untracked { mutable: true }, true))
        );
        // The static constants' origin is immutable, and another field of
        // the static origin is no named origin.
        let static_shape: String = STATIC_CONSTANTS_ORIGIN.split_whitespace().collect();
        assert_eq!(
            Origin::from_lit_attribute(&format!("{static_shape}:!lit.origin<{{}}>"), Some(true)),
            None
        );
        assert_eq!(
            Origin::from_lit_attribute(
                "#lit.origin.field<#lit.static.origin:!lit.origin<false>,\"x\">:!lit.origin<false>",
                None
            ),
            None
        );
        assert_eq!(Origin::Param(OriginParamId(0)).lit_attribute(true), None);
    }

    #[test]
    fn unions_are_flattened_deduplicated_and_ordered() {
        let a = place(1, &[]);
        let b = place(2, &[]);
        assert_eq!(
            Origin::union([b.clone(), Origin::union([a.clone(), b])]),
            Origin::Union(vec![a, place(2, &[])])
        );
    }

    #[test]
    fn concrete_capture_sets_are_ordered_and_write_subsumes_read() {
        let first = place(1, &[]);
        let second = place(2, &[]);
        assert_eq!(
            CaptureOriginSet::concrete([
                CaptureOrigin::read(second.clone()),
                CaptureOrigin::read(first.clone()),
                CaptureOrigin::write(first.clone()),
                CaptureOrigin::read(Origin::union([second.clone(), first.clone()])),
            ]),
            CaptureOriginSet(vec![
                CaptureOrigin::write(first),
                CaptureOrigin::read(second),
            ])
        );
    }

    #[test]
    fn signature_unions_order_projected_members_canonically() {
        let left = SigOrigin::Projected(
            Box::new(SigOrigin::Param(0)),
            vec![OriginSeg::Field("value".into())],
        );
        let right = SigOrigin::Projected(
            Box::new(SigOrigin::Param(1)),
            vec![OriginSeg::Field("value".into())],
        );
        assert_eq!(
            SigOrigin::union([right.clone(), left.clone(), right]),
            SigOrigin::Union(vec![
                left,
                SigOrigin::Projected(
                    Box::new(SigOrigin::Param(1)),
                    vec![OriginSeg::Field("value".into())],
                )
            ])
        );
    }

    #[test]
    fn projected_places_use_prefix_and_wildcard_overlap() {
        let field_a = OriginSeg::Field("a".into());
        let field_b = OriginSeg::Field("b".into());
        assert!(place(1, &[]).overlaps(&place(1, std::slice::from_ref(&field_a))));
        assert!(!place(1, std::slice::from_ref(&field_a)).overlaps(&place(1, &[field_b])));
        assert!(place(1, &[OriginSeg::AnyIndex]).overlaps(&place(1, &[field_a])));
        assert!(!place(1, &[]).overlaps(&place(2, &[])));
    }

    #[test]
    fn clone_binder_re_applies_an_interior_or_subtree_projection() {
        let pointer = |path: &[OriginSeg]| PointerOrigin::Place {
            place: OriginPlace {
                root: OwnerId(1),
                path: path.to_vec(),
            },
            mutable: true,
        };
        let field = OriginSeg::Field("items".into());
        let element = OriginSeg::Interior("element".into());
        let projected = pointer(&[field.clone(), element.clone(), OriginSeg::Subtree]);
        let projection = projected.clone_bindable_place().expect("bindable");
        assert_eq!(projection.interior, ["element"]);
        assert!(projection.subtree);
        assert_eq!(
            projected.without_projection(&projection.interior, projection.subtree),
            Some(pointer(std::slice::from_ref(&field)))
        );
        assert_eq!(projected.without_projection(&[], false), Some(projected));
        assert!(
            pointer(std::slice::from_ref(&element))
                .without_projection(&[], true)
                .is_none()
        );
        assert!(pointer(&[element, field]).clone_bindable_place().is_none());
    }

    #[test]
    fn pointer_origins_round_trip_through_their_struct_argument_parts() {
        let id = OriginParamId(3);
        let mutabilities = [
            Mutability::Mutable,
            Mutability::Immutable,
            Mutability::Param(OriginParamId(1)),
        ];
        let projections = [
            (Vec::new(), false),
            (vec!["element".to_string()], false),
            (vec!["bytes".to_string(), "element".to_string()], false),
            (Vec::new(), true),
            (vec!["element".to_string()], true),
        ];
        let mut pointers = Vec::new();
        for mutable in [false, true] {
            pointers.push(PointerOrigin::Place {
                place: OriginPlace {
                    root: OwnerId(1),
                    path: vec![OriginSeg::Field("a".into()), OriginSeg::Subtree],
                },
                mutable,
            });
            pointers.push(PointerOrigin::Static { mutable });
            pointers.push(PointerOrigin::Untracked { mutable });
            pointers.push(PointerOrigin::UnsafeAny { mutable });
        }
        for mutability in mutabilities {
            for (interior, subtree) in &projections {
                pointers.push(PointerOrigin::Param {
                    id,
                    mutability,
                    interior: interior.clone(),
                    subtree: *subtree,
                });
                pointers.push(PointerOrigin::SelfPlace {
                    mutability,
                    interior: interior.clone(),
                    subtree: *subtree,
                });
            }
        }
        for pointer in pointers {
            let (origin, mutability) = pointer.parts();
            assert_eq!(
                PointerOrigin::from_parts(origin, mutability),
                Some(pointer.clone()),
                "{pointer:?}"
            );
        }
        assert_eq!(
            PointerOrigin::from_parts(Origin::immutable(Origin::Param(id)), Mutability::Mutable),
            Some(PointerOrigin::Param {
                id,
                mutability: Mutability::Immutable,
                interior: Vec::new(),
                subtree: false,
            })
        );
        assert!(PointerOrigin::from_parts(Origin::Unbound, Mutability::Immutable).is_none());
    }

    #[test]
    fn projections_extend_places_and_wrap_binders_but_not_untracked_origins() {
        let binder = Origin::Param(OriginParamId(0));
        let element = OriginSeg::Interior("element".into());
        let interior = binder.owned_interior("element").expect("binder interior");
        assert_eq!(
            interior,
            Origin::Projected(Box::new(binder.clone()), vec![element.clone()])
        );
        let subtree = interior.subtree().expect("interior subtree");
        assert_eq!(
            subtree,
            Origin::Projected(
                Box::new(binder.clone()),
                vec![element.clone(), OriginSeg::Subtree]
            )
        );
        assert!(subtree.owned_interior("x").is_none());
        assert!(subtree.subtree().is_none());
        assert_eq!(
            Origin::SelfParam.subtree(),
            Some(Origin::Projected(
                Box::new(Origin::SelfParam),
                vec![OriginSeg::Subtree]
            ))
        );
        assert_eq!(
            place(1, &[]).owned_interior("element"),
            Some(place(1, std::slice::from_ref(&element)))
        );
        assert_eq!(
            Origin::immutable(binder.clone()).owned_interior("element"),
            Some(Origin::immutable(interior.clone()))
        );
        assert_eq!(
            Origin::union([binder.clone(), place(1, &[])]).owned_interior("element"),
            Some(Origin::union([interior.clone(), place(1, &[element])]))
        );
        for untracked in [
            Origin::Static,
            Origin::Untracked { mutable: true },
            Origin::UnsafeAny { mutable: false },
            Origin::Unbound,
        ] {
            assert!(untracked.owned_interior("element").is_none());
            assert!(untracked.subtree().is_none());
        }
        // A `_subtree` slot accepts the storage it projects, never the reverse,
        // and an interior projection is a distinct origin.
        let binder_subtree = binder.subtree().expect("binder subtree");
        assert!(binder.coerces_to(&binder_subtree));
        assert!(!binder_subtree.coerces_to(&binder));
        assert!(!binder.coerces_to(&interior));
        assert!(interior.coerces_to(&subtree));
        assert!(!Origin::UnsafeAny { mutable: true }.coerces_to(&binder));
    }

    #[test]
    fn interior_tags_are_distinct_but_remain_under_their_base() {
        let elements = OriginSeg::Interior("element".into());
        let values = OriginSeg::Interior("value".into());
        assert!(place(1, &[]).overlaps(&place(1, std::slice::from_ref(&elements))));
        assert!(!place(1, &[elements]).overlaps(&place(1, &[values])));
    }

    #[test]
    fn subtree_overlaps_every_descendant_of_its_base() {
        let field_a = OriginSeg::Field("a".into());
        let field_b = OriginSeg::Field("b".into());
        let subtree_at_a = place(1, &[field_a.clone(), OriginSeg::Subtree]);
        // The base itself, and anything below it, overlaps the subtree.
        assert!(subtree_at_a.overlaps(&place(1, std::slice::from_ref(&field_a))));
        assert!(subtree_at_a.overlaps(&place(
            1,
            &[field_a.clone(), field_b.clone(), OriginSeg::AnyIndex]
        )));
        assert!(subtree_at_a.overlaps(&place(
            1,
            &[field_a.clone(), OriginSeg::Interior("element".into())]
        )));
        // A sibling path and a foreign root stay disjoint.
        assert!(!subtree_at_a.overlaps(&place(1, std::slice::from_ref(&field_b))));
        assert!(!subtree_at_a.overlaps(&place(2, &[field_a])));
    }
}
