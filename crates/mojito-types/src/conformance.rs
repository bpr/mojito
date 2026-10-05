//! Conformance of a compiler-known type to a built-in trait.
//!
//! A struct answers by its declaration and a type parameter by its bounds;
//! both belong to whoever holds those. What is left — the scalars, SIMD
//! values, tuples, callables — conforms by its shape alone, so the checker
//! and the elaborator ask the same predicate here.

use crate::types::{Ty, default_literal, is_scalar_simd, simd_shape};

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
