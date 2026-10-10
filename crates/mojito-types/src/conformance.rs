//! Conformance of a compiler-known type to a built-in trait.
//!
//! A struct answers by its declaration and a type parameter by its bounds;
//! both belong to whoever holds those. What is left — the scalars, SIMD
//! values, tuples, callables — conforms by its shape alone, so the checker
//! and the elaborator ask the same predicate here.

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
    "Sized",
    "SizedRaising",
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
        "Writable" | "Hasher" | "Hashable" | "Defaultable" | "Indexer" | "Equatable"
        | "Comparable" | "Intable" | "Floatable" => StructTraitRule::Declared,
        "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable" | "Multipliable"
        | "Divisible" | "FloorDivisible" | "Modable" | "ShiftLeftable" | "ShiftRightable"
        | "Andable" | "Orable" | "Xorable" | "Negatable" => StructTraitRule::Never,
        _ => StructTraitRule::Shallow,
    }
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
        ty,
        Ty::Struct(..)
            | Ty::Param { .. }
            | Ty::Assoc { .. }
            | Ty::Dependent(_)
            | Ty::SelfType
            | Ty::Infer
            | Ty::Error
    ) {
        return None;
    }
    Some(match trait_name {
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" => match ty {
            Ty::ComptimeList(item) => element(item, trait_name),
            Ty::Tuple(items) | Ty::RuntimePack(items) | Ty::Variant(items) => {
                items.iter().all(|item| element(item, trait_name))
            }
            _ => true,
        },
        "Hashable" => builtin_hashable_ty(ty),
        "Writable" => !matches!(
            ty,
            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
        ),
        "Writer" | "Hasher" => false,
        "Defaultable" => match ty {
            Ty::Int
            | Ty::IntLiteral
            | Ty::UInt
            | Ty::Float64
            | Ty::FloatLiteral
            | Ty::Bool
            | Ty::StringLiteral
            | Ty::None
            | Ty::Simd { .. } => true,
            Ty::Tuple(items) | Ty::RuntimePack(items) => {
                items.iter().all(|item| element(item, trait_name))
            }
            _ => false,
        },
        "Indexer" => matches!(ty, Ty::Int | Ty::IntLiteral),
        "Equatable" => is_scalar(ty) || is_numeric_like(ty) || is_scalar_simd(ty),
        "Comparable" => is_numeric_like(ty) || is_scalar_simd(ty),
        "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable" | "Multipliable"
        | "Divisible" | "FloorDivisible" | "Modable" | "Floatable" => is_numeric_like(ty),
        "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
            is_integer_like(ty)
        }
        "Negatable" => is_signed_numeric_like(ty),
        "Intable" => is_numeric_like(ty) || *ty == Ty::Bool,
        // `AnyType`, and the layout and operation markers that stay shallow.
        _ => true,
    })
}

/// Whether `ty` is a non-numeric scalar value type — what `==`/`!=` compare once
/// the numeric cases are out of the way.
pub const fn is_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool | Ty::StringLiteral | Ty::None | Ty::Dtype)
}

/// Whether `ty` is a numeric type (concrete or literal).
pub const fn is_numeric(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int | Ty::UInt | Ty::Float64 | Ty::IntLiteral | Ty::FloatLiteral
    )
}

pub fn is_numeric_like(ty: &Ty) -> bool {
    is_numeric(&default_literal(ty))
}

/// Integer-kind scalars — the operands of bitwise and shift operators
/// (`IntLiteral` materializes to `Int`).
pub fn is_integer_like(ty: &Ty) -> bool {
    matches!(default_literal(ty), Ty::Int | Ty::UInt)
}

/// Signed numeric scalars — the operands of arithmetic negation (`-x`); `UInt`
/// is excluded.
pub fn is_signed_numeric_like(ty: &Ty) -> bool {
    matches!(default_literal(ty), Ty::Int | Ty::Float64)
}

pub const fn builtin_hashable_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int
            | Ty::UInt
            | Ty::Bool
            | Ty::StringLiteral
            | Ty::Float64
            | Ty::Simd { .. }
            | Ty::Dtype
    )
}

/// Whether `ty` is a SIMD value — a native scalar (a width-1 vector) or a
/// `SIMD[dtype, width]` — the types the hidden `$SIMD` bound admits.
pub const fn simd_valued_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Simd { .. }) || simd_shape(ty).is_some()
}
