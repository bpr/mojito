//! Conformance of a compiler-known type to a built-in trait.
//!
//! A struct answers by its declaration and a type parameter by its bounds;
//! both belong to whoever holds those. What is left — the scalars, SIMD
//! values, tuples, callables — conforms by its shape alone, so the checker
//! and the elaborator ask the same predicate here.

use crate::types::TyKind;
use crate::types::{Ty, default_literal, is_scalar_simd, simd_shape, trivial_predicate_name};

/// Mojo's built-in traits the compiler knows by name. A program may also
/// declare a trait of one of these names; its declaration then decides.
pub const COMPILER_TRAITS: &[&str] = &[
    "AnyType",
    "Deinitable",
    "Movable",
    "Copyable",
    "ImplicitlyCopyable",
    "RegisterPassable",
    "TrivialRegisterPassable",
    "Defaultable",
    "Representable",
    "Writable",
    "Writer",
    "Boolable",
    "Intable",
    "Floatable",
    "Indexer",
    "Equatable",
    "Comparable",
    "Hashable",
    "Hasher",
    "Identifiable",
    "Iterable",
    "IterableOwned",
    "Iterator",
    "Absable",
    "Powable",
    "Roundable",
    "Ceilable",
    "Floorable",
    "Truncable",
    "CeilDivable",
    "CeilDivableRaising",
    "DivModable",
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
    /// Always: a compiler-known trait that stays shallow.
    Shallow,
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
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" => StructTraitRule::Lifecycle,
        "Writable"
        | "Hasher"
        | "Hashable"
        | "Defaultable"
        | "Indexer"
        | "Equatable"
        | "Comparable"
        | "Intable"
        | "Floatable"
        | "RegisterPassable"
        | "TrivialRegisterPassable" => StructTraitRule::Declared,
        "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable" | "Multipliable"
        | "Divisible" | "FloorDivisible" | "Modable" | "ShiftLeftable" | "ShiftRightable"
        | "Andable" | "Orable" | "Xorable" | "Negatable" => StructTraitRule::Never,
        _ => StructTraitRule::Shallow,
    }
}

/// Whether the compiler-known trait `candidate` refines the compiler-known
/// `required`, as upstream declares `TrivialRegisterPassable` a refinement
/// of `RegisterPassable`, where no program declares either.
pub fn compiler_trait_refines(candidate: &str, required: &str) -> bool {
    matches!(
        (candidate, required),
        ("TrivialRegisterPassable", "RegisterPassable")
    )
}

/// Whether the trait `candidate` refines `required`, directly or through
/// its parents, where `parents` answers a declared trait's parents. A
/// refinement cycle proves nothing.
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
        visiting.insert(candidate.to_string())
            && parents(candidate).is_some_and(|parents_of| {
                parents_of
                    .iter()
                    .any(|parent| parent == required || walk(parents, parent, required, visiting))
            })
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
/// The pin answers so for an MLIR type (`!kgen.string`), not even
/// `AnyType`. A trait-bounded binder still binds one, and `conforms_to`
/// over that binder answers by the type's shape, as at the pin.
pub fn reflects_no_conformance(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::KgenString)
}

/// Whether the shape of `ty` alone decides its conformance to the built-in
/// trait `trait_name`, and how.
///
/// `None` for a type whose answer is a declaration's: a struct, a type
/// parameter, an associated or dependent type. `element` answers for a
/// component of an aggregate, which may be such a type.
pub fn leaf_conforms(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> bool,
) -> Option<bool> {
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
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" | "RegisterPassable" => {
            match ty.kind() {
                TyKind::ComptimeList(item) | TyKind::PopArray { element: item, .. } => {
                    element(item, trait_name)
                }
                TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
                    items.iter().all(|item| element(item, trait_name))
                }
                // A closure's storage is copyable exactly when each of its
                // captures is.
                TyKind::Func(func) if matches!(trait_name, "Copyable" | "ImplicitlyCopyable") => {
                    match &func.environment {
                        crate::origin::CallableEnvironment::Storage(storage) => storage.copyable,
                        _ => true,
                    }
                }
                _ => true,
            }
        }
        "Hashable" => builtin_hashable_ty(ty),
        "Writable" => !matches!(
            ty.kind(),
            TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
        ),
        "Writer" | "Hasher" => false,
        "Defaultable" => match ty.kind() {
            TyKind::Int
            | TyKind::IntLiteral
            | TyKind::UInt
            | TyKind::Float64
            | TyKind::FloatLiteral
            | TyKind::Bool
            | TyKind::StringLiteral
            | TyKind::None
            | TyKind::Simd { .. } => true,
            TyKind::Tuple(items) | TyKind::RuntimePack(items) => {
                items.iter().all(|item| element(item, trait_name))
            }
            _ => false,
        },
        "Indexer" => matches!(ty.kind(), TyKind::Int | TyKind::IntLiteral),
        "Equatable" => is_scalar(ty) || is_numeric_like(ty) || is_scalar_simd(ty),
        "Comparable" => is_numeric_like(ty) || is_scalar_simd(ty),
        "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable" | "Multipliable"
        | "Divisible" | "FloorDivisible" | "Modable" | "Floatable" => is_numeric_like(ty),
        "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
            is_integer_like(ty)
        }
        "Negatable" => is_signed_numeric_like(ty),
        "Intable" => is_numeric_like(ty) || *ty == TyKind::Bool,
        // `AnyType`, and the layout and operation markers that stay shallow.
        _ => true,
    })
}

/// Whether the compiler-modelled type `ty` conforms to the library trait
/// `trait_name`, as the upstream struct it stands in for declares.
///
/// Upstream's `SIMD` (whose scalars `Int`, `UInt`, and `Float64` are),
/// `Tuple`, `VariadicPack`, and `TypeList` each list `Sized`. A struct or a
/// type parameter answers by its own declaration or bounds instead.
pub fn modelled_declares(ty: &Ty, trait_name: &str) -> bool {
    match trait_name {
        "Sized" => matches!(
            ty.kind(),
            TyKind::Int
                | TyKind::UInt
                | TyKind::Float64
                | TyKind::Simd { .. }
                | TyKind::Tuple(_)
                | TyKind::RuntimePack(_)
                | TyKind::VariadicPack(_)
                | TyKind::ComptimeList(_)
        ),
        _ => false,
    }
}

/// Whether `ty` is a non-numeric scalar value type — what `==`/`!=` compare once
/// the numeric cases are out of the way.
pub fn is_scalar(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Bool | TyKind::StringLiteral | TyKind::None | TyKind::Dtype
    )
}

/// Whether `ty` is a numeric type (concrete or literal).
pub fn is_numeric(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int | TyKind::UInt | TyKind::Float64 | TyKind::IntLiteral | TyKind::FloatLiteral
    )
}

pub fn is_numeric_like(ty: &Ty) -> bool {
    is_numeric(&default_literal(ty))
}

/// Integer-kind scalars — the operands of bitwise and shift operators
/// (`IntLiteral` materializes to `Int`).
pub fn is_integer_like(ty: &Ty) -> bool {
    matches!(default_literal(ty).kind(), TyKind::Int | TyKind::UInt)
}

/// Signed numeric scalars — the operands of arithmetic negation (`-x`); `UInt`
/// is excluded.
pub fn is_signed_numeric_like(ty: &Ty) -> bool {
    matches!(default_literal(ty).kind(), TyKind::Int | TyKind::Float64)
}

pub fn builtin_hashable_ty(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int
            | TyKind::UInt
            | TyKind::Bool
            | TyKind::StringLiteral
            | TyKind::Float64
            | TyKind::Simd { .. }
            | TyKind::Dtype
    )
}

/// Whether `ty` is a SIMD value — a native scalar (a width-1 vector) or a
/// `SIMD[dtype, width]` — the types the hidden `$SIMD` bound admits.
pub fn simd_valued_ty(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Simd { .. }) || simd_shape(ty).is_some()
}
