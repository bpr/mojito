//! The runtime C ABI contract table: the single source of truth for every
//! symbol and `#[repr(C)]` type the `mojito-runtime` crate exports.
//!
//! Three artifacts must stay in lockstep — this table, the `mojito-runtime`
//! implementation, and the normative `docs/native-abi.md` contract — and the
//! agreement is mechanical: the default-lane `tests/native_abi_test.rs`
//! checks the runtime crate's Rust signatures and layouts against this table,
//! and the pliron-lane cross checks validate the backend's emitted LLVM
//! declarations and target-data layouts against the same rows. Bump
//! [`MJRT_ABI_VERSION`] on any change to a row.
//!
//! The runtime ABI passes only primitives and pointers by value; aggregates
//! cross as pointers. Nothing here may reference the VM's internal `Value`
//! representation.

use crate::native::layout::{Layout, StructLayout, compose};
use crate::native::target::NativeTarget;

/// The runtime ABI version this compiler emits against. Must equal
/// `mojito_runtime::ABI_VERSION`; the linked runtime exports it as the
/// inspectable data symbol [`ABI_VERSION_SYMBOL`].
pub const MJRT_ABI_VERSION: u32 = 1;

/// The exported `u32` data symbol carrying the runtime's ABI version.
pub const ABI_VERSION_SYMBOL: &str = "mjrt_abi_version";

/// Trap categories passed to `mjrt_trap` and encoded in trap-block exit
/// codes (`64 + category`). Values 1 and 2 match the backend's existing
/// `TrapCategory` codes.
pub const TRAP_DIV_MOD_ZERO: u32 = 1;
pub const TRAP_POW_EXPONENT: u32 = 2;
pub const TRAP_ALLOC_FAILURE: u32 = 3;
pub const TRAP_STDOUT_FAILURE: u32 = 4;

/// Tag values of tagged success/error outcomes.
pub const MJ_TAG_OK: u32 = 0;
pub const MJ_TAG_ERR: u32 = 1;

/// A primitive of the runtime C ABI. Exported functions pass and return only
/// these; aggregates cross the boundary as pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CAbiTy {
    U32,
    U64,
    I64,
    F64,
    PtrConstU8,
    PtrMutU8,
}

impl CAbiTy {
    pub fn layout(self, target: &NativeTarget) -> Layout {
        match self {
            CAbiTy::U32 => Layout::new(4, 4),
            CAbiTy::U64 | CAbiTy::I64 | CAbiTy::F64 => Layout::new(8, 8),
            CAbiTy::PtrConstU8 | CAbiTy::PtrMutU8 => {
                Layout::new(target.triple.pointer_size(), target.triple.pointer_align())
            }
        }
    }
}

/// The complete contract of one exported runtime function.
#[derive(Debug, Clone, Copy)]
pub struct RtSig {
    pub symbol: &'static str,
    pub params: &'static [(&'static str, CAbiTy)],
    /// `None` for `void` and for functions that never return.
    pub ret: Option<CAbiTy>,
    pub noreturn: bool,
    /// The ABI version that introduced this symbol.
    pub since: u32,
    /// Who owns pointed-to memory, and nullability of every pointer.
    pub ownership: &'static str,
    /// What happens when the operation cannot complete.
    pub failure: &'static str,
}

/// Every function symbol the runtime exports, in stable declaration order.
pub const RT_SYMBOLS: &[RtSig] = &[
    RtSig {
        symbol: "mjrt_version",
        params: &[],
        ret: Some(CAbiTy::U32),
        noreturn: false,
        since: 1,
        ownership: "no pointers",
        failure: "never fails",
    },
    RtSig {
        symbol: "mjrt_alloc",
        params: &[("size", CAbiTy::U64), ("align", CAbiTy::U64)],
        ret: Some(CAbiTy::PtrMutU8),
        noreturn: false,
        since: 1,
        ownership: "caller owns the result; release via mjrt_dealloc with the \
                    same size/align; zero size returns an aligned dangling \
                    pointer; never null",
        failure: "traps (allocation-failure) on exhaustion or invalid align",
    },
    RtSig {
        symbol: "mjrt_dealloc",
        params: &[
            ("ptr", CAbiTy::PtrMutU8),
            ("size", CAbiTy::U64),
            ("align", CAbiTy::U64),
        ],
        ret: None,
        noreturn: false,
        since: 1,
        ownership: "consumes an mjrt_alloc allocation; null ptr or zero size \
                    is a no-op",
        failure: "undefined on a size/align mismatch (contract violation)",
    },
    RtSig {
        symbol: "mjrt_write_stdout",
        params: &[("data", CAbiTy::PtrConstU8), ("len", CAbiTy::U64)],
        ret: None,
        noreturn: false,
        since: 1,
        ownership: "borrows data for the call; data may be null only when len \
                    is 0",
        failure: "traps (stdout-failure) on a write error; retries interrupts",
    },
    RtSig {
        symbol: "mjrt_fmt_i64",
        params: &[("value", CAbiTy::I64), ("out", CAbiTy::PtrMutU8)],
        ret: Some(CAbiTy::U64),
        noreturn: false,
        since: 1,
        ownership: "borrows out (>= 20 writable bytes, caller-owned); returns \
                    bytes written, no NUL terminator",
        failure: "never fails",
    },
    RtSig {
        symbol: "mjrt_fmt_u64",
        params: &[("value", CAbiTy::U64), ("out", CAbiTy::PtrMutU8)],
        ret: Some(CAbiTy::U64),
        noreturn: false,
        since: 1,
        ownership: "borrows out (>= 20 writable bytes, caller-owned); returns \
                    bytes written, no NUL terminator",
        failure: "never fails",
    },
    RtSig {
        symbol: "mjrt_fmt_f64",
        params: &[("value", CAbiTy::F64), ("out", CAbiTy::PtrMutU8)],
        ret: Some(CAbiTy::U64),
        noreturn: false,
        since: 1,
        ownership: "borrows out (>= 32 writable bytes, caller-owned); returns \
                    bytes written, no NUL terminator; text matches the VM's \
                    Float64 display",
        failure: "never fails",
    },
    RtSig {
        symbol: "mjrt_trap",
        params: &[("category", CAbiTy::U32)],
        ret: None,
        noreturn: true,
        since: 1,
        ownership: "no pointers",
        failure: "always: reports on stderr and exits with 64 + category \
                  (clamped to 127)",
    },
];

/// Exported data symbols, in stable declaration order.
pub const RT_DATA_SYMBOLS: &[(&str, CAbiTy)] = &[(ABI_VERSION_SYMBOL, CAbiTy::U32)];

/// A field of an exported `#[repr(C)]` type: a primitive or a reference to
/// another entry of [`RT_TYPES`] by name.
#[derive(Debug, Clone, Copy)]
pub enum RtFieldTy {
    Prim(CAbiTy),
    Named(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct RtField {
    pub name: &'static str,
    pub ty: RtFieldTy,
}

/// The complete layout contract of one exported `#[repr(C)]` type.
#[derive(Debug, Clone, Copy)]
pub struct RtTypeSpec {
    pub name: &'static str,
    pub fields: &'static [RtField],
}

/// Every `#[repr(C)]` type the runtime exports, in stable declaration order.
/// Each entry may reference only earlier entries by name.
pub const RT_TYPES: &[RtTypeSpec] = &[
    RtTypeSpec {
        name: "MjStrDesc",
        fields: &[
            RtField {
                name: "data",
                ty: RtFieldTy::Prim(CAbiTy::PtrConstU8),
            },
            RtField {
                name: "len",
                ty: RtFieldTy::Prim(CAbiTy::U64),
            },
        ],
    },
    RtTypeSpec {
        name: "MjString",
        fields: &[
            RtField {
                name: "data",
                ty: RtFieldTy::Prim(CAbiTy::PtrMutU8),
            },
            RtField {
                name: "size",
                ty: RtFieldTy::Prim(CAbiTy::I64),
            },
            RtField {
                name: "cap",
                ty: RtFieldTy::Prim(CAbiTy::I64),
            },
        ],
    },
    RtTypeSpec {
        name: "MjError",
        fields: &[RtField {
            name: "message",
            ty: RtFieldTy::Named("MjString"),
        }],
    },
];

/// Look up a function contract by symbol name.
pub fn find_symbol(symbol: &str) -> Option<&'static RtSig> {
    RT_SYMBOLS.iter().find(|sig| sig.symbol == symbol)
}

/// Look up an exported type contract by name.
pub fn find_type(name: &str) -> Option<&'static RtTypeSpec> {
    RT_TYPES.iter().find(|spec| spec.name == name)
}

/// The target layout of an exported type, resolving named field references
/// through [`RT_TYPES`]. Panics on a dangling name — the table is const data
/// whose integrity the default-lane tests pin.
pub fn type_layout(spec: &RtTypeSpec, target: &NativeTarget) -> StructLayout {
    let fields: Vec<Layout> = spec
        .fields
        .iter()
        .map(|field| match field.ty {
            RtFieldTy::Prim(prim) => prim.layout(target),
            RtFieldTy::Named(name) => {
                let inner = find_type(name)
                    .unwrap_or_else(|| panic!("RT_TYPES references unknown type '{name}'"));
                type_layout(inner, target).layout
            }
        })
        .collect();
    compose(&fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::target::{NativeTarget, Triple};

    #[test]
    fn table_lookups_and_named_references_resolve() {
        assert!(find_symbol("mjrt_alloc").is_some());
        assert!(find_symbol("mjrt_missing").is_none());
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        for spec in RT_TYPES {
            // Resolves without panicking and yields a nonzero layout.
            let layout = type_layout(spec, &target);
            assert!(layout.layout.size > 0, "{}", spec.name);
        }
    }

    #[test]
    fn exported_type_layouts_are_pinned() {
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        let desc = type_layout(find_type("MjStrDesc").unwrap(), &target);
        assert_eq!((desc.layout.size, desc.layout.align), (16, 8));
        assert_eq!(desc.offsets, vec![0, 8]);
        let string = type_layout(find_type("MjString").unwrap(), &target);
        assert_eq!((string.layout.size, string.layout.align), (24, 8));
        assert_eq!(string.offsets, vec![0, 8, 16]);
        let error = type_layout(find_type("MjError").unwrap(), &target);
        assert_eq!((error.layout.size, error.layout.align), (24, 8));
        assert_eq!(error.offsets, vec![0]);
    }

    #[test]
    fn noreturn_symbols_return_nothing() {
        for sig in RT_SYMBOLS {
            if sig.noreturn {
                assert!(sig.ret.is_none(), "{}", sig.symbol);
            }
            assert!(sig.since <= MJRT_ABI_VERSION, "{}", sig.symbol);
            assert!(sig.symbol.starts_with("mjrt_"), "{}", sig.symbol);
        }
    }
}
