//! The names the VM answers without a program function.
//!
//! These are the direct callees its builtin dispatch handles and the
//! methods of compiler-private values it implements itself. A consumer
//! that has to decide whether a call target resolves (the A1 shadow core's
//! legality rule) reads these tables rather than keeping its own copy; the
//! tests below pin each name to the dispatch source that implements it.

/// The prefix of the symbols the VM dispatches dynamically on a trait bound.
///
/// A call target under it (`__trait_dispatch.copy$ov$`) resolves at run
/// time by the receiver's kind, never to one declaration.
pub const TRAIT_DISPATCH_PREFIX: &str = "__trait_dispatch.";

/// The direct callees `backend::vm::dispatch` handles without a program
/// function, in the order of its arms.
pub const BUILTIN_CALLEES: &[&str] = &[
    "range",
    "print",
    "external_call",
    "_mojito_abort",
    "String",
    "repr",
    "len",
    "Slice",
    "slice",
    "abs",
    "min",
    "max",
    "round",
    "input",
    "Int",
    "Float64",
    "Bool",
    "UInt",
    "divmod",
    "Error",
    "UnsafePointer.alloc",
    "UnsafePointer.alloc_aligned",
    "UnsafePointer.unsafe_dangling",
    "Pointer.unsafe_dangling",
];

/// The methods of compiler-private values a method call may name without
/// a checker-selected symbol.
///
/// They are the methods of strings, packs, slices, SIMD vectors, `DType`,
/// pointers, and hashable scalars: the arms of
/// `backend::vm::invoke::method_call` that run before struct dispatch,
/// `runtime::simd_method`, and the `DType` predicates.
pub const INTRINSIC_METHODS: &[&str] = &[
    "__hash__",
    "_update_with_simd",
    "__floor__",
    "__ceil__",
    "__trunc__",
    "__ceildiv__",
    "__fma__",
    "format",
    "write_string",
    "byte_length",
    "ptr",
    "unsafe_ptr",
    "write",
    "__len__",
    "indices",
    "__eq__",
    "__ne__",
    "copy",
    "free",
    "unsafe_free",
    "lt",
    "le",
    "gt",
    "ge",
    "eq",
    "ne",
    "select",
    "reduce_add",
    "reduce_mul",
    "reduce_min",
    "reduce_max",
    "reduce_and",
    "reduce_or",
    "is_integral",
    "is_floating_point",
    "is_signed",
    "is_unsigned",
    "is_numeric",
    "is_float8",
    "is_half_float",
];

#[cfg(test)]
mod tests {
    use super::*;

    const DISPATCH: &str = include_str!("backend/vm/dispatch.rs");
    const INVOKE: &str = include_str!("backend/vm/invoke.rs");
    const RUNTIME: &str = include_str!("runtime.rs");

    fn quoted(source: &str, name: &str) -> bool {
        source.contains(&format!("\"{name}\""))
    }

    #[test]
    fn every_builtin_callee_has_a_dispatch_arm() {
        for name in BUILTIN_CALLEES {
            assert!(quoted(DISPATCH, name), "`{name}` has no arm in dispatch.rs");
        }
    }

    #[test]
    fn every_intrinsic_method_has_an_implementation() {
        for name in INTRINSIC_METHODS {
            let implemented = quoted(INVOKE, name)
                || quoted(RUNTIME, name)
                || mojito_ast::ast::Dtype::Int.predicate(name).is_some();
            assert!(
                implemented,
                "`{name}` is implemented nowhere the VM dispatches"
            );
        }
    }
}
