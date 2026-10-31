//! Conformance of a compiler-known type to a built-in trait.
//!
//! A struct answers by its declaration and a type parameter by its bounds;
//! both belong to whoever holds those. What is left — the scalars, SIMD
//! values, tuples, callables — conforms by its shape alone, so the checker
//! and the elaborator ask the same predicate here.

use crate::types::TyKind;
use crate::types::{Ty, default_literal, simd_shape, trivial_predicate_name};

/// Mojo's built-in traits the compiler knows by name. A program may also
/// declare a trait of one of these names; its declaration then decides.
pub const COMPILER_TRAITS: &[&str] = &[
    "AnyType",
    "Deinitable",
    "Movable",
    "Copyable",
    "ImplicitlyCopyable",
    "Representable",
    "Writer",
    "Iterable",
    "IterableOwned",
    "Iterator",
    "Addable",
    "Subtractable",
    "Multipliable",
    "Divisible",
    "FloorDivisible",
    "Modable",
    "ShiftLeftable",
    "ShiftRightable",
    "Andable",
    "Orable",
    "Xorable",
    "Negatable",
];

/// A conformance query's answer, upstream's `TriBool`: an archetype whose
/// bounds neither provide nor exclude a trait is unknown, not refuted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conformance {
    Proven,
    Refuted,
    Unknown,
}

impl Conformance {
    pub fn is_proven(self) -> bool {
        self == Self::Proven
    }

    /// Both hold: a refutation decides, then an unknown.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Refuted, _) | (_, Self::Refuted) => Self::Refuted,
            (Self::Proven, Self::Proven) => Self::Proven,
            _ => Self::Unknown,
        }
    }

    /// Either holds: a proof decides, then an unknown.
    #[must_use]
    pub const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Proven, _) | (_, Self::Proven) => Self::Proven,
            (Self::Refuted, Self::Refuted) => Self::Refuted,
            _ => Self::Unknown,
        }
    }
}

impl From<bool> for Conformance {
    fn from(holds: bool) -> Self {
        if holds { Self::Proven } else { Self::Refuted }
    }
}

/// The traits a struct's declaration answers beside its declared list.
///
/// The lifecycle traits, which its methods and fields decide as well, and
/// the `IsTrivially*` spelling of each trivial facet.
pub const LIFECYCLE_TRAITS: [&str; 7] = [
    "Copyable",
    "ImplicitlyCopyable",
    "Movable",
    "Deinitable",
    "IsTriviallyCopyable",
    "IsTriviallyMovable",
    "IsTriviallyDeinitable",
];

/// How a struct's conformance to a trait is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructTraitRule {
    /// By the struct's synthesized lifecycle answer.
    Lifecycle,
    /// By its declared conformances: a declared trait that is the trait or
    /// refines it, under that declaration's condition.
    Declared,
    /// Never: an operation trait no struct conforms to.
    Never,
    /// Always: `AnyType`, which every type conforms to.
    Always,
}

/// The rule deciding whether a struct conforms to `trait_name`, which the
/// program declares when `declared_by_program`.
pub fn struct_trait_rule(trait_name: &str, declared_by_program: bool) -> StructTraitRule {
    if trivial_predicate_name(trait_name).is_some() {
        return StructTraitRule::Lifecycle;
    }
    if declared_by_program || !COMPILER_TRAITS.contains(&trait_name) {
        return StructTraitRule::Declared;
    }
    match trait_name {
        "AnyType" => StructTraitRule::Always,
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" => StructTraitRule::Lifecycle,
        "Addable" | "Subtractable" | "Multipliable" | "Divisible" | "FloorDivisible"
        | "Modable" | "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable"
        | "Negatable" => StructTraitRule::Never,
        _ => StructTraitRule::Declared,
    }
}

/// The parents upstream's `std.traits` declares for the compiler-known
/// lifecycle trait `name`, which no declaration answers here:
/// `Copyable(Movable)` and `ImplicitlyCopyable(Copyable)`.
pub fn compiler_trait_parents(name: &str) -> &'static [&'static str] {
    match name {
        "Copyable" => &["Movable"],
        "ImplicitlyCopyable" => &["Copyable"],
        _ => &[],
    }
}

/// Whether the trait `candidate` refines `required`, directly or through
/// its parents.
///
/// `parents` answers a declared trait's parents and
/// [`compiler_trait_parents`] a compiler-known one's. A refinement cycle
/// proves nothing.
pub fn refines<'a>(
    parents: &dyn Fn(&str) -> Option<&'a [String]>,
    candidate: &str,
    required: &str,
) -> bool {
    fn walk<'a>(
        parents: &dyn Fn(&str) -> Option<&'a [String]>,
        candidate: &str,
        required: &str,
        visiting: &mut std::collections::HashSet<String>,
    ) -> bool {
        if !visiting.insert(candidate.to_string()) {
            return false;
        }
        let mut reaches =
            |parent: &str| parent == required || walk(parents, parent, required, visiting);
        match parents(candidate) {
            Some(declared) => declared.iter().any(|parent| reaches(parent)),
            None => compiler_trait_parents(candidate)
                .iter()
                .any(|parent| reaches(parent)),
        }
    }
    walk(
        parents,
        candidate,
        required,
        &mut std::collections::HashSet::new(),
    )
}

/// Whether a closed `conforms_to(ty, Tr)` is `False` for every trait.
///
/// The pin answers so for an MLIR type (`!kgen.string`, `!kgen.deferred`,
/// `index`, `i1`, `!kgen.simd`, `!kgen.pointer`, `!lit.ref`), not even
/// `AnyType`. A trait-bounded binder still binds one, and `conforms_to` over
/// that binder answers by the type's shape, as at the pin.
pub fn reflects_no_conformance(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::KgenString
            | TyKind::KgenDeferred
            | TyKind::KgenTarget
            | TyKind::Index
            | TyKind::I1
            | TyKind::KgenSimd { .. }
            | TyKind::KgenPointer(_)
            | TyKind::LitRef { .. }
    )
}

/// Whether the shape of `ty` alone decides its conformance to the built-in
/// trait `trait_name`, and how.
///
/// `None` for a type whose answer is a declaration's: a struct, a type
/// parameter, an associated or dependent type. `element` answers for a
/// component of an aggregate, which may be such a type.
pub fn leaf_conformance(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> Conformance,
) -> Option<Conformance> {
    if matches!(
        ty.kind(),
        TyKind::Struct(..)
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::Dependent(_)
            | TyKind::SelfType
            | TyKind::Infer
            | TyKind::Error
    ) {
        return None;
    }
    Some(match trait_name {
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" => {
            match ty.kind() {
                TyKind::ComptimeList(item) | TyKind::PopArray { element: item, .. } => {
                    element(item, trait_name)
                }
                TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
                    every(items, trait_name, element)
                }
                // A closure's storage is copyable exactly when each of its
                // captures is.
                TyKind::Func(func) if matches!(trait_name, "Copyable" | "ImplicitlyCopyable") => {
                    match &func.environment {
                        crate::origin::CallableEnvironment::Storage(storage) => {
                            storage.copyable.into()
                        }
                        _ => Conformance::Proven,
                    }
                }
                _ => Conformance::Proven,
            }
        }
        "Writer" => Conformance::Refuted,
        "Addable" | "Subtractable" | "Multipliable" | "Divisible" | "FloorDivisible"
        | "Modable" => is_numeric_like(ty).into(),
        "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
            is_integer_like(ty).into()
        }
        "Negatable" => is_signed_numeric_like(ty).into(),
        // `AnyType`, and the layout and operation markers that stay shallow.
        _ => Conformance::Proven,
    })
}

/// [`leaf_conformance`] where only a proof counts.
pub fn leaf_conforms(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> bool,
) -> Option<bool> {
    leaf_conformance(ty, trait_name, &mut |item, trait_name| {
        element(item, trait_name).into()
    })
    .map(Conformance::is_proven)
}

/// How the compiler-modelled type `ty` conforms to the library trait
/// `trait_name`, as the upstream struct it stands in for declares.
///
/// Upstream's `Tuple`, `VariadicPack`, and `TypeList` each list `Sized`.
/// `Bool` and the numeric literals list `Equatable` and `Comparable`, and
/// `DType` lists `Equatable`.
///
/// `Bool` and the literal structs list `Boolable`; `Bool` and the numeric
/// literals `Intable`; `Bool` and `FloatLiteral` `Floatable`; `IntLiteral`
/// `Indexer`; none lists `Identifiable`. `SIMD`'s `Boolable`, `Comparable`,
/// `Equatable`, `Intable`, `Indexer`, and `Sized` are its declaration's in
/// `std.simd`, which the checker and the elaborator read instead.
///
/// `SIMD` lists every `std.math` operation trait but `CeilDivableRaising`;
/// its `Absable`, `Ceilable`, `Floorable`, `Powable`, `Roundable`, and
/// `Truncable` are its declaration's, and `IntLiteral` lists `Ceilable`,
/// `Floorable`, and `Truncable`. For `DivModable` and `CeilDivable` a
/// numeric literal answers as the scalar it materializes to, and only
/// `Int`, `UInt`, and `Float64` conform, since the backends compute those
/// operations over no other `SIMD`.
///
/// `Bool`, `StringLiteral`, and `DType` list `Hashable` (a `SIMD`'s is its
/// declaration's); none lists `Hasher`.
///
/// `SIMD`, `Bool`, the literals, and `NoneType` list `Defaultable`, and
/// `SIMD`, `Bool`, the numeric literals, `NoneType`, and `DType`
/// `TrivialRegisterPassable`, which refines `RegisterPassable`; `Tuple`
/// lists `Defaultable` and `RegisterPassable` where every element
/// conforms, which `element` answers. A thin function is
/// `TrivialRegisterPassable`, and a closure's storage `RegisterPassable`
/// or `TrivialRegisterPassable` where each of its captures is.
///
/// Every modelled leaf but a callable lists `Writable`; `Tuple` and
/// `Variant` list it where every element conforms, which `element` answers.
///
/// A struct or a type parameter answers by its own declaration or bounds
/// instead.
pub fn modelled_conformance(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> Conformance,
) -> Conformance {
    // The `std.math.math` traits outside the prelude answer by their linked
    // name.
    let declares = match trait_name
        .strip_prefix("__module$std$math$math$")
        .unwrap_or(trait_name)
    {
        "Boolable" => matches!(
            ty.kind(),
            TyKind::Bool | TyKind::StringLiteral(_) | TyKind::IntLiteral | TyKind::FloatLiteral
        ),
        "Intable" => matches!(
            ty.kind(),
            TyKind::Bool | TyKind::IntLiteral | TyKind::FloatLiteral | TyKind::Pointer { .. }
        ),
        "Floatable" => matches!(ty.kind(), TyKind::Bool | TyKind::FloatLiteral),
        "Indexer" => *ty == TyKind::IntLiteral,
        "Equatable" => matches!(
            ty.kind(),
            TyKind::Bool | TyKind::Dtype | TyKind::IntLiteral | TyKind::FloatLiteral
        ),
        "Comparable" => matches!(
            ty.kind(),
            TyKind::Bool | TyKind::IntLiteral | TyKind::FloatLiteral
        ),
        "Ceilable" | "Floorable" | "Truncable" => *ty == TyKind::IntLiteral,
        "Absable" | "Powable" | "Roundable" => false,
        "DivModable" | "CeilDivable" => is_numeric_like(ty),
        "Hashable" => builtin_hashable_ty(ty),
        "Writable" => match ty.kind() {
            TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
                return every(items, trait_name, element);
            }
            TyKind::Func(_)
            | TyKind::GenericFunc(_)
            | TyKind::Overload(_)
            | TyKind::Index
            | TyKind::I1
            | TyKind::KgenSimd { .. }
            | TyKind::KgenPointer(_)
            | TyKind::LitRef { .. } => false,
            _ => true,
        },
        "Defaultable" => match ty.kind() {
            TyKind::Tuple(items) | TyKind::RuntimePack(items) => {
                return every(items, trait_name, element);
            }
            _ => {
                is_numeric_like(ty)
                    || matches!(
                        ty.kind(),
                        TyKind::Bool | TyKind::StringLiteral(_) | TyKind::None
                    )
            }
        },
        "TrivialRegisterPassable" => modelled_trivial_register_passable(ty),
        "RegisterPassable" => match ty.kind() {
            TyKind::Tuple(items) | TyKind::RuntimePack(items) => {
                return every(items, trait_name, element);
            }
            TyKind::Func(func) if !thin_callable(ty) => matches!(
                &func.environment,
                crate::origin::CallableEnvironment::Storage(storage) if storage.register_passable
            ),
            _ => modelled_trivial_register_passable(ty),
        },
        "Sized" => matches!(
            ty.kind(),
            TyKind::Tuple(_)
                | TyKind::RuntimePack(_)
                | TyKind::VariadicPack(_)
                | TyKind::ComptimeList(_)
        ),
        _ => false,
    };
    declares.into()
}

/// [`modelled_conformance`] where only a proof counts.
pub fn modelled_declares(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> bool,
) -> bool {
    modelled_conformance(ty, trait_name, &mut |item, trait_name| {
        element(item, trait_name).into()
    })
    .is_proven()
}

/// Whether `ty` is a non-numeric scalar value type — what `==`/`!=` compare once
/// the numeric cases are out of the way.
pub fn is_scalar(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Bool | TyKind::StringLiteral(_) | TyKind::None | TyKind::Dtype
    )
}

/// Whether `ty` is a numeric type (concrete or literal).
pub fn is_numeric(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::INT | TyKind::UINT | TyKind::FLOAT64 | TyKind::IntLiteral | TyKind::FloatLiteral
    )
}

pub fn is_numeric_like(ty: &Ty) -> bool {
    is_numeric(&default_literal(ty))
}

/// Integer-kind scalars — the operands of bitwise and shift operators
/// (`IntLiteral` materializes to `Int`).
pub fn is_integer_like(ty: &Ty) -> bool {
    matches!(default_literal(ty).kind(), TyKind::INT | TyKind::UINT)
}

/// Signed numeric scalars — the operands of arithmetic negation (`-x`); `UInt`
/// is excluded.
pub fn is_signed_numeric_like(ty: &Ty) -> bool {
    matches!(default_literal(ty).kind(), TyKind::INT | TyKind::FLOAT64)
}

pub fn builtin_hashable_ty(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Bool | TyKind::StringLiteral(_) | TyKind::Dtype
    )
}

/// Whether `ty` is a SIMD value — a native scalar (a width-1 vector) or a
/// `SIMD[dtype, width]` — the types the hidden `$SIMD` bound admits.
pub fn simd_valued_ty(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Simd { .. }) || simd_shape(ty).is_some()
}

/// Whether `ty` is a thin function's type, generic or not: a bare function
/// pointer, which upstream passes in a register and copies bitwise.
pub fn thin_callable(ty: &Ty) -> bool {
    ty.kind()
        .callable()
        .is_some_and(|func| matches!(func.environment, crate::origin::CallableEnvironment::Thin))
}

/// Whether the compiler-modelled `ty` stands in for an upstream struct
/// declaring `TrivialRegisterPassable`, or is a type upstream gives that
/// conformance: a `SIMD` value, `Bool`, a numeric literal, `NoneType`,
/// `DType`, a thin function, or a closure's storage whose captures all are.
fn modelled_trivial_register_passable(ty: &Ty) -> bool {
    is_numeric_like(ty)
        || simd_valued_ty(ty)
        || thin_callable(ty)
        || ty.kind().as_func().is_some_and(|func| {
            matches!(
                &func.environment,
                crate::origin::CallableEnvironment::Storage(storage)
                    if storage.trivially_register_passable
            )
        })
        || matches!(
            ty.kind(),
            TyKind::Bool
                | TyKind::None
                | TyKind::Dtype
                | TyKind::Index
                | TyKind::I1
                | TyKind::KgenSimd { .. }
                | TyKind::KgenPointer(_)
                | TyKind::LitRef { .. }
        )
}

/// Whether every one of `items` conforms to `trait_name`, as `element`
/// answers each.
fn every(
    items: &[Ty],
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> Conformance,
) -> Conformance {
    items
        .iter()
        .try_fold(Conformance::Proven, |all, item| {
            match all.and(element(item, trait_name)) {
                Conformance::Refuted => None,
                all => Some(all),
            }
        })
        .unwrap_or(Conformance::Refuted)
}
