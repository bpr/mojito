//! The names the VM answers without a program function.
//!
//! These are the direct callees its builtin dispatch handles and the
//! methods of compiler-private values it implements itself. A consumer
//! that has to decide whether a call target resolves (the A1 shadow core's
//! legality rule) reads these tables rather than keeping its own copy; the
//! tests below pin each name to the dispatch source that implements it.

/// The direct callees `backend::vm::dispatch` handles without a program
/// function, in the order of its arms.
pub const BUILTIN_CALLEES: &[&str] = &[
    "range",
    "external_call",
    "_mojito_abort",
    "_mojito_float_text",
    "len",
    "abs",
    "min",
    "max",
    "input",
    "Error",
    "UnsafePointer.alloc",
    "UnsafePointer.alloc_aligned",
    "UnsafePointer.unsafe_dangling",
    "Pointer.unsafe_dangling",
];

/// The methods of compiler-private values a method call may name without
/// a checker-selected symbol.
///
/// They are the methods of strings, packs, SIMD vectors, and pointers: the
/// arms of `backend::vm::invoke::method_call` that run before struct
/// dispatch.
pub const INTRINSIC_METHODS: &[&str] = &[
    "__floor__",
    "__ceil__",
    "__trunc__",
    "write_string",
    mojito_symbol::symbol::WRITE_FORMATTED,
    mojito_symbol::symbol::WRITE_REPR_FORMATTED,
    "__len__",
    "copy",
    "free",
    "unsafe_free",
];

#[cfg(test)]
mod tests {
    use super::*;

    const DISPATCH: &str = include_str!("backend/vm/dispatch.rs");
    const EXEC: &str = include_str!("backend/vm/exec.rs");
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
                || quoted(EXEC, name)
                || quoted(RUNTIME, name)
                || (mojito_symbol::symbol::is_formatted_write(name)
                    && INVOKE.contains("mojito_symbol::symbol::is_formatted_write"));
            assert!(
                implemented,
                "`{name}` is implemented nowhere the VM dispatches"
            );
        }
    }
}
