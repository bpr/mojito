//! Canonical checked representation for origins and reference permissions.
//!
//! Source origin clauses are expressions because the parser must preserve
//! syntax it cannot resolve. Past the checker boundary, origins use stable
//! binding identities and deliberately lose source names.

use std::cmp::Ordering;

/// Stable identity of a value binding in one checker run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerId(pub u32);

/// Stable identity of a declared or inferred origin parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OriginParamId(pub u32);

/// Stable identity of an `OriginSet` parameter appearing in a callable
/// environment contract. Like [`OriginParamId`], this is the declaration-order
/// identity selected by checking rather than a source spelling, so independently
/// named but otherwise identical callable contracts remain alpha-equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaptureSetParamId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Stable owner plus its statically known projection path.
pub struct OriginPlace {
    pub root: OwnerId,
    pub path: Vec<OriginSeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Canonical checked description of storage a reference may designate.
pub enum Origin {
    Param(OriginParamId),
    Place(OriginPlace),
    Union(Vec<Origin>),
    Static,
    Untracked { mutable: bool },
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Origin::Param(id) => write!(f, "origin#{}", id.0),
            Origin::Place(place) => {
                write!(f, "origin_of(#{}", place.root.0)?;
                for segment in &place.path {
                    match segment {
                        OriginSeg::Field(name) => write!(f, ".{name}")?,
                        OriginSeg::AnyIndex => write!(f, "[_]")?,
                        OriginSeg::Interior(tag) => write!(f, ".<{tag}>")?,
                    }
                }
                write!(f, ")")
            }
            Origin::Union(origins) => {
                for (index, origin) in origins.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{origin}")?;
                }
                Ok(())
            }
            Origin::Static => write!(f, "StaticOrigin"),
            Origin::Untracked { mutable } => {
                write!(
                    f,
                    "{}",
                    if *mutable {
                        "UnsafeAnyOrigin"
                    } else {
                        "UntrackedOrigin"
                    }
                )
            }
        }
    }
}

/// Access performed through one captured origin when a callable is invoked.
/// This is deliberately part of the static environment summary: an OriginSet
/// by itself extends lifetimes but cannot tell persistent-loan analysis whether
/// an indirect call reads or may mutate captured storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureAccess {
    Read,
    Write,
}

/// One canonical concrete dependency in a capturing callable environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptureOrigin {
    pub origin: Origin,
    pub access: CaptureAccess,
}

impl CaptureOrigin {
    pub fn read(origin: Origin) -> Self {
        Self {
            origin,
            access: CaptureAccess::Read,
        }
    }

    pub fn write(origin: Origin) -> Self {
        Self {
            origin,
            access: CaptureAccess::Write,
        }
    }
}

/// The origin dependencies of a parametric capturing callable.
///
/// Mojo's `OriginSet` is deliberately separate from an [`Origin`] union: it is
/// the set of owner origins retained by a callable environment, not an origin a
/// reference value can designate. `Infer` is the checked placeholder introduced
/// by `capturing[_]`; `Param` names a source `OriginSet` binder after resolution;
/// and `Concrete` is the canonical set attached to a materialized environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CaptureOriginSet {
    Infer,
    Param(CaptureSetParamId),
    Concrete(Vec<CaptureOrigin>),
}

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
                    }))
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
        Self::Concrete(canonical)
    }

    pub fn empty() -> Self {
        Self::Concrete(Vec::new())
    }
}

/// How a checked callable relates to an environment.
///
/// `Default` is the unqualified `def(...)` callable constraint. It is not an
/// implicit wildcard coercion: later bound solving may explicitly accept a
/// concrete environment for such a constraint, but ordinary value coercion
/// compares this fact exactly. `Thin` is an explicitly non-capturing function;
/// `Capturing` retains its `OriginSet` contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum CallableEnvironment {
    #[default]
    Default,
    Thin,
    Capturing(CaptureOriginSet),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Permission available through a checked reference.
pub enum Mutability {
    Immutable,
    Mutable,
    Param(OriginParamId),
}

/// Provenance retained by an origin-bearing unsafe pointer type.  `Legacy`
/// represents Mojito's one-argument compatibility spelling; all current-Mojo
/// spellings remain explicit through checked HIR/MIR and are erased only by the
/// VM value representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PointerOrigin {
    Legacy,
    Place {
        place: OriginPlace,
        mutable: bool,
    },
    Param {
        id: OriginParamId,
        mutability: Mutability,
    },
    Static,
    Untracked {
        mutable: bool,
    },
    UnsafeAny {
        mutable: bool,
    },
}

impl PointerOrigin {
    /// The loan-tracked [`Origin`] a pointer provenance corresponds to, when it
    /// designates checked storage. `Legacy`, `Static`, `Untracked`, and
    /// `UnsafeAny` pointers carry no owner loan.
    pub fn as_origin(&self) -> Option<Origin> {
        match self {
            PointerOrigin::Place { place, .. } => Some(Origin::Place(place.clone())),
            PointerOrigin::Param { id, .. } => Some(Origin::Param(*id)),
            PointerOrigin::Legacy
            | PointerOrigin::Static
            | PointerOrigin::Untracked { .. }
            | PointerOrigin::UnsafeAny { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Checked reference type: referent, storage origin, and access permission.
pub struct RefTy {
    pub referent: Box<crate::types::Ty>,
    pub origin: Origin,
    pub mutability: Mutability,
}

/// An origin in a callable contract. Unlike [`Origin`], roots name parameter
/// slots rather than checker-local bindings, so the contract survives overload
/// storage and can be substituted with caller places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigOrigin {
    Self_,
    Param(usize),
    /// A concrete caller-side origin captured by an explicitly specialized
    /// function value. Unlike `Param`, this contract is no longer relative to
    /// the eventual call's argument slots.
    Bound(Origin),
    Static,
    Untracked {
        mutable: bool,
    },
    Projected(Box<SigOrigin>, Vec<OriginSeg>),
    Union(Vec<SigOrigin>),
    Infer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
/// Reference result or parameter contract retained in a callable signature.
pub struct RefSig {
    pub origin: SigOrigin,
    pub mutability: SigMutability,
}

impl Origin {
    /// Construct a canonical union: flatten nesting, remove duplicates, and
    /// sort members independently of source order.
    pub fn union(origins: impl IntoIterator<Item = Origin>) -> Origin {
        let mut members = Vec::new();
        for origin in origins {
            match origin {
                Origin::Union(inner) => members.extend(inner),
                other => members.push(other),
            }
        }
        members.sort_by(cmp_origin);
        members.dedup();
        match members.len() {
            0 => Origin::Union(Vec::new()),
            1 => members.pop().expect("one union member"),
            _ => Origin::Union(members),
        }
    }

    /// Whether two origins may designate overlapping storage.
    pub fn overlaps(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Union(left), right) => left.iter().any(|item| item.overlaps(right)),
            (left, Origin::Union(right)) => right.iter().any(|item| left.overlaps(item)),
            (Origin::Place(left), Origin::Place(right)) => places_overlap(left, right),
            (Origin::Param(left), Origin::Param(right)) => left == right,
            (Origin::Static, Origin::Static) => true,
            // Untracked origins intentionally forfeit disjointness information.
            (Origin::Untracked { .. }, _) | (_, Origin::Untracked { .. }) => true,
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
    pub fn union(origins: impl IntoIterator<Item = SigOrigin>) -> SigOrigin {
        let mut members = Vec::new();
        for origin in origins {
            match canonical_sig_origin(origin) {
                SigOrigin::Union(inner) => members.extend(inner),
                other => members.push(other),
            }
        }
        members.sort_by(cmp_sig_origin);
        members.dedup();
        match members.len() {
            0 => SigOrigin::Union(Vec::new()),
            1 => members.pop().expect("one signature-origin union member"),
            _ => SigOrigin::Union(members),
        }
    }
}

/// Whether two projected owner places may designate overlapping storage.
pub fn places_overlap(left: &OriginPlace, right: &OriginPlace) -> bool {
    if left.root != right.root {
        return false;
    }
    left.path.iter().zip(&right.path).all(|(a, b)| {
        a == b || matches!(a, OriginSeg::AnyIndex) || matches!(b, OriginSeg::AnyIndex)
    })
}

fn cmp_origin(left: &Origin, right: &Origin) -> Ordering {
    fn tag(origin: &Origin) -> u8 {
        match origin {
            Origin::Param(_) => 0,
            Origin::Place(_) => 1,
            Origin::Static => 2,
            Origin::Untracked { .. } => 3,
            Origin::Union(_) => 4,
        }
    }
    tag(left)
        .cmp(&tag(right))
        .then_with(|| match (left, right) {
            (Origin::Param(a), Origin::Param(b)) => a.cmp(b),
            (Origin::Place(a), Origin::Place(b)) => a.cmp(b),
            (Origin::Untracked { mutable: a }, Origin::Untracked { mutable: b }) => a.cmp(b),
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
    fn tag(origin: &SigOrigin) -> u8 {
        match origin {
            SigOrigin::Self_ => 0,
            SigOrigin::Param(_) => 1,
            SigOrigin::Bound(_) => 2,
            SigOrigin::Static => 3,
            SigOrigin::Untracked { .. } => 4,
            SigOrigin::Projected(_, _) => 5,
            SigOrigin::Infer => 6,
            SigOrigin::Union(_) => 7,
        }
    }

    tag(left)
        .cmp(&tag(right))
        .then_with(|| match (left, right) {
            (SigOrigin::Param(a), SigOrigin::Param(b)) => a.cmp(b),
            (SigOrigin::Bound(a), SigOrigin::Bound(b)) => cmp_origin(a, b),
            (SigOrigin::Untracked { mutable: a }, SigOrigin::Untracked { mutable: b }) => a.cmp(b),
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
            _ => Ordering::Equal,
        })
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
            CaptureOriginSet::Concrete(vec![
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
    fn interior_tags_are_distinct_but_remain_under_their_base() {
        let elements = OriginSeg::Interior("element".into());
        let values = OriginSeg::Interior("value".into());
        assert!(place(1, &[]).overlaps(&place(1, std::slice::from_ref(&elements))));
        assert!(!place(1, &[elements]).overlaps(&place(1, &[values])));
    }
}
