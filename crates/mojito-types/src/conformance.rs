//! Conformance of a compiler-known type to a built-in trait.
//!
//! A struct answers by its declaration and a type parameter by its bounds;
//! both belong to whoever holds those. What is left — the scalars, SIMD
//! values, tuples, callables — conforms by its shape alone, so the checker
//! and the elaborator ask the same predicate here.

use crate::types::TyKind;
use crate::types::{
    Ty, default_literal, is_scalar_simd, scalar_simd_dtype, simd_shape, trivial_predicate_name,
};

/// Mojo's built-in traits the compiler knows by name. A program may also
/// declare a trait of one of these names; its declaration then decides.
pub const COMPILER_TRAITS: &[&str] = &[
    "AnyType",
    "Deinitable",
    "Movable",
    "Copyable",
    "ImplicitlyCopyable",
    "Representable",
    "Writable",
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
        "Copyable" | "ImplicitlyCopyable" | "Movable" | "Deinitable" => {
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
        "Writable" => !matches!(
            ty.kind(),
            TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
        ),
        "Writer" => false,
        "Addable" | "Subtractable" | "Multipliable" | "Divisible" | "FloorDivisible"
        | "Modable" => is_numeric_like(ty),
        "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
            is_integer_like(ty)
        }
        "Negatable" => is_signed_numeric_like(ty),
        // `AnyType`, and the layout and operation markers that stay shallow.
        _ => true,
    })
}

/// Whether the compiler-modelled type `ty` conforms to the library trait
/// `trait_name`, as the upstream struct it stands in for declares.
///
/// Upstream's `SIMD` (whose scalars `Int`, `UInt`, and `Float64` are),
/// `Tuple`, `VariadicPack`, and `TypeList` each list `Sized`. `SIMD` and
/// `Bool` list `Equatable` and `Comparable`, and `DType` lists `Equatable`;
/// a numeric literal answers as the scalar it materializes to. Only a
/// scalar `SIMD` conforms here, since a vector's comparison through the
/// bound is not yet its reducing `__eq__`.
///
/// `SIMD`, `Bool`, and the literal structs list `Boolable`; `SIMD`, `Bool`,
/// and the numeric literals `Intable`; `Bool` and `FloatLiteral`
/// `Floatable`; `SIMD` and `IntLiteral` `Indexer`; none lists
/// `Identifiable`. Only a scalar `SIMD` is `Intable` here, and only an
/// integral one `Indexer`, since upstream's methods assert as much where
/// an instance calls them.
///
/// `SIMD` lists every `std.math` operation trait but `CeilDivableRaising`;
/// a numeric literal answers as the scalar it materializes to. Only `Int`,
/// `UInt`, and `Float64` conform here, and a float scalar `SIMD` to
/// `Ceilable`, `Floorable`, and `Truncable`, since the backends compute
/// the operations over no other `SIMD` (R1457).
///
/// `SIMD`, `Bool`, `StringLiteral`, and `DType` list `Hashable`; none lists
/// `Hasher`.
///
/// `SIMD`, `Bool`, the literals, and `NoneType` list `Defaultable`, and
/// `SIMD`, `Bool`, the numeric literals, `NoneType`, and `DType`
/// `TrivialRegisterPassable`, which refines `RegisterPassable`; `Tuple`
/// lists `Defaultable` and `RegisterPassable` where every element
/// conforms, which `element` answers. A closure's storage is
/// `RegisterPassable` where each of its captures is.
///
/// A struct or a type parameter answers by its own declaration or bounds
/// instead.
pub fn modelled_declares(
    ty: &Ty,
    trait_name: &str,
    element: &mut dyn FnMut(&Ty, &str) -> bool,
) -> bool {
    // The `std.math` traits outside the prelude answer by their linked name.
    match trait_name
        .strip_prefix("__module$std$math$")
        .unwrap_or(trait_name)
    {
        "Boolable" => {
            is_numeric_like(ty)
                || matches!(
                    ty.kind(),
                    TyKind::Bool | TyKind::StringLiteral | TyKind::Simd { .. }
                )
        }
        "Intable" => is_numeric_like(ty) || is_scalar_simd(ty) || *ty == TyKind::Bool,
        "Floatable" => matches!(ty.kind(), TyKind::Bool | TyKind::FloatLiteral),
        "Indexer" => {
            is_integer_like(ty)
                || scalar_simd_dtype(ty).is_some_and(mojito_ast::ast::Dtype::is_integral)
        }
        "Equatable" => {
            is_numeric_like(ty)
                || is_scalar_simd(ty)
                || matches!(ty.kind(), TyKind::Bool | TyKind::Dtype)
        }
        "Comparable" => is_numeric_like(ty) || is_scalar_simd(ty) || *ty == TyKind::Bool,
        "Ceilable" | "Floorable" | "Truncable" => {
            is_numeric_like(ty)
                || scalar_simd_dtype(ty).is_some_and(mojito_ast::ast::Dtype::is_float)
        }
        "Absable" | "Powable" | "Roundable" | "DivModable" | "CeilDivable" => is_numeric_like(ty),
        "Hashable" => builtin_hashable_ty(ty),
        "Defaultable" => match ty.kind() {
            TyKind::Tuple(items) | TyKind::RuntimePack(items) => {
                items.iter().all(|item| element(item, trait_name))
            }
            _ => {
                is_numeric_like(ty)
                    || matches!(
                        ty.kind(),
                        TyKind::Bool | TyKind::StringLiteral | TyKind::None | TyKind::Simd { .. }
                    )
            }
        },
        "TrivialRegisterPassable" => modelled_trivial_register_passable(ty),
        "RegisterPassable" => match ty.kind() {
            TyKind::Tuple(items) | TyKind::RuntimePack(items) => {
                items.iter().all(|item| element(item, trait_name))
            }
            TyKind::Func(func) => matches!(
                &func.environment,
                crate::origin::CallableEnvironment::Storage(storage) if storage.register_passable
            ),
            _ => modelled_trivial_register_passable(ty),
        },
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

/// Whether the compiler-modelled `ty` stands in for an upstream struct
/// declaring `TrivialRegisterPassable`: a `SIMD` value, `Bool`, a numeric
/// literal, `NoneType`, or `DType`.
fn modelled_trivial_register_passable(ty: &Ty) -> bool {
    is_numeric_like(ty)
        || simd_valued_ty(ty)
        || matches!(ty.kind(), TyKind::Bool | TyKind::None | TyKind::Dtype)
}
