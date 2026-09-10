//! The closed libc callee table behind the `external_call` builtin.
//!
//! Mojito accepts upstream's `external_call[callee, return_type, ...](*args)`
//! spelling only for the callees listed here (a strict subset of Mojo's
//! open-ended FFI): the checker rejects any other callee name, validates
//! each argument and the declared return type against the row, the VM
//! executes the row in Rust, and the native backend declares the row as an
//! external C function and calls it. All three phases read this one table
//! so the accepted shapes cannot drift.
//!
//! The rows describe the Linux x86-64 glibc ABI (the native target); the VM
//! reproduces the same observable behavior — return values, `errno`, and the
//! byte images written through pointer arguments — on top of Rust's standard
//! library.

use crate::types::Ty;
use mojito_ast::ast::Dtype;

/// One C parameter or return kind.
///
/// Integer kinds name the C type so both backends widen or narrow Mojo scalars
/// to it; pointer kinds are one opaque address at the ABI and differ only in
/// what the checker accepts and what the VM reads or writes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CType {
    /// `int`.
    Int,
    /// `unsigned int` / `mode_t`.
    UInt,
    /// `size_t`.
    SizeT,
    /// `ssize_t`.
    SSizeT,
    /// `off_t`.
    OffT,
    /// `const char *`: a NUL-terminated string (a `CStringSlice` or a
    /// `Pointer` to `Int8`/`UInt8` bytes).
    ConstCharPtr,
    /// `char *`: a byte buffer the callee writes, or a NUL-terminated
    /// string the callee returns.
    CharPtr,
    /// `void *`: any pointer (a byte buffer, a struct the callee fills, or
    /// an opaque handle the callee returned earlier).
    VoidPtr,
    /// `int *` (`__errno_location`).
    IntPtr,
    /// No return value.
    Void,
}

impl CType {
    /// Whether the kind is passed and returned as a C integer.
    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Int | Self::UInt | Self::SizeT | Self::SSizeT | Self::OffT
        )
    }

    /// Whether the kind is an address.
    pub const fn is_pointer(self) -> bool {
        matches!(
            self,
            Self::ConstCharPtr | Self::CharPtr | Self::VoidPtr | Self::IntPtr
        )
    }

    /// The C ABI width in bits of an integer kind (`int` is 32-bit, the
    /// `size_t` family 64-bit on the LP64 target).
    pub const fn bits(self) -> u32 {
        match self {
            Self::Int | Self::UInt => 32,
            _ => 64,
        }
    }

    /// The type vocabulary a diagnostic names for the kind.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Int => "a C int (Int32 or an integer scalar)",
            Self::UInt => "a C unsigned int (UInt32 or an integer scalar)",
            Self::SizeT => "a C size_t (UInt or an integer scalar)",
            Self::SSizeT => "a C ssize_t (Int or an integer scalar)",
            Self::OffT => "a C off_t (Int64 or an integer scalar)",
            Self::ConstCharPtr => "a C string (CStringSlice or Pointer[Int8]/Pointer[UInt8])",
            Self::CharPtr => "a byte Pointer",
            Self::VoidPtr => "a Pointer",
            Self::IntPtr => "Pointer[Int32, _]",
            Self::Void => "NoneType",
        }
    }
}

/// One allowlisted libc function: its symbol, fixed parameters, the
/// optional trailing variadic parameters (`open`'s `mode`), the return kind,
/// and whether a pointer return may be null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiCallee {
    pub name: &'static str,
    pub params: &'static [CType],
    pub variadic_extra: &'static [CType],
    pub ret: CType,
    pub nullable_ret: bool,
}

impl FfiCallee {
    /// Whether the C declaration is variadic (`int open(const char *, int, ...)`).
    pub const fn variadic(&self) -> bool {
        !self.variadic_extra.is_empty()
    }

    /// The parameter kind at argument position `index`, counting the
    /// variadic tail after the fixed parameters.
    pub fn param(&self, index: usize) -> Option<CType> {
        self.params
            .get(index)
            .or_else(|| self.variadic_extra.get(index - self.params.len()))
            .copied()
    }

    /// The accepted argument-count range.
    pub const fn arity(&self) -> (usize, usize) {
        let fixed = self.params.len();
        (fixed, fixed + self.variadic_extra.len())
    }
}

/// Every callee `external_call` accepts, in stable order. The rows mirror
/// the glibc prototypes the pinned upstream stdlib calls.
pub const CALLEES: &[FfiCallee] = &[
    FfiCallee {
        name: "open",
        params: &[CType::ConstCharPtr, CType::Int],
        variadic_extra: &[CType::UInt],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "read",
        params: &[CType::Int, CType::VoidPtr, CType::SizeT],
        variadic_extra: &[],
        ret: CType::SSizeT,
        nullable_ret: false,
    },
    FfiCallee {
        name: "write",
        params: &[CType::Int, CType::VoidPtr, CType::SizeT],
        variadic_extra: &[],
        ret: CType::SSizeT,
        nullable_ret: false,
    },
    FfiCallee {
        name: "close",
        params: &[CType::Int],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "lseek",
        params: &[CType::Int, CType::OffT, CType::Int],
        variadic_extra: &[],
        ret: CType::OffT,
        nullable_ret: false,
    },
    FfiCallee {
        name: "unlink",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "mkdir",
        params: &[CType::ConstCharPtr, CType::UInt],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "rmdir",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "opendir",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::VoidPtr,
        nullable_ret: true,
    },
    FfiCallee {
        name: "readdir",
        params: &[CType::VoidPtr],
        variadic_extra: &[],
        ret: CType::CharPtr,
        nullable_ret: true,
    },
    FfiCallee {
        name: "closedir",
        params: &[CType::VoidPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "getcwd",
        params: &[CType::CharPtr, CType::SizeT],
        variadic_extra: &[],
        ret: CType::CharPtr,
        nullable_ret: true,
    },
    FfiCallee {
        name: "getenv",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::CharPtr,
        nullable_ret: true,
    },
    FfiCallee {
        name: "setenv",
        params: &[CType::ConstCharPtr, CType::ConstCharPtr, CType::Int],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "unsetenv",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "strerror",
        params: &[CType::Int],
        variadic_extra: &[],
        ret: CType::CharPtr,
        nullable_ret: false,
    },
    FfiCallee {
        name: "__errno_location",
        params: &[],
        variadic_extra: &[],
        ret: CType::IntPtr,
        nullable_ret: false,
    },
    FfiCallee {
        name: "memcpy",
        params: &[CType::VoidPtr, CType::VoidPtr, CType::SizeT],
        variadic_extra: &[],
        ret: CType::VoidPtr,
        nullable_ret: false,
    },
    FfiCallee {
        name: "strlen",
        params: &[CType::ConstCharPtr],
        variadic_extra: &[],
        ret: CType::SizeT,
        nullable_ret: false,
    },
    FfiCallee {
        name: "__xstat",
        params: &[CType::Int, CType::ConstCharPtr, CType::VoidPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
    FfiCallee {
        name: "__lxstat",
        params: &[CType::Int, CType::ConstCharPtr, CType::VoidPtr],
        variadic_extra: &[],
        ret: CType::Int,
        nullable_ret: false,
    },
];

/// The row for `name`, when it is allowlisted.
pub fn callee(name: &str) -> Option<&'static FfiCallee> {
    CALLEES.iter().find(|row| row.name == name)
}

/// The allowlisted names, comma-separated, for diagnostics.
pub fn callee_names() -> String {
    CALLEES
        .iter()
        .map(|row| row.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a checked argument type may be passed at a parameter of kind
/// `kind`.
///
/// Integer kinds take `Int`, `UInt`, `Bool`, an integer literal, or any
/// width-1 integer SIMD scalar (the backends resize to the C width); pointer
/// kinds take any `Pointer` (a byte element for `const char *`) or the
/// `CStringSlice` view struct.
pub fn accepts_arg(kind: CType, ty: &Ty) -> bool {
    match kind {
        CType::Int | CType::UInt | CType::SizeT | CType::SSizeT | CType::OffT => {
            is_integer_scalar(ty)
        }
        CType::ConstCharPtr => match ty {
            Ty::Pointer { element, .. } => is_byte(element),
            Ty::Struct(name, _) => is_c_string_slice_struct(name),
            _ => false,
        },
        CType::CharPtr | CType::VoidPtr | CType::IntPtr => match ty {
            Ty::Pointer { .. } => true,
            Ty::Struct(name, _) => is_c_string_slice_struct(name),
            _ => false,
        },
        CType::Void => false,
    }
}

/// Whether the declared `return_type` may receive a result of kind `kind`:
/// integer kinds need an integer scalar type, pointer kinds a `Pointer`
/// (`Pointer[Int32, _]` for `int *`), `Void` the unit type.
pub fn accepts_ret(kind: CType, ty: &Ty) -> bool {
    match kind {
        CType::Int | CType::UInt | CType::SizeT | CType::SSizeT | CType::OffT => {
            matches!(ty, Ty::Int | Ty::UInt)
                || matches!(ty, Ty::Simd { width: 1, dtype } if is_integer_dtype(*dtype))
        }
        CType::IntPtr => matches!(ty, Ty::Pointer { element, .. }
            if matches!(**element, Ty::Simd { dtype: Dtype::Int32 | Dtype::UInt32, width: 1 })),
        CType::ConstCharPtr | CType::CharPtr | CType::VoidPtr => {
            matches!(ty, Ty::Pointer { .. })
        }
        CType::Void => matches!(ty, Ty::None),
    }
}

/// Whether `name` is the stdlib `CStringSlice` view (qualified or bare).
pub fn is_c_string_slice_struct(name: &str) -> bool {
    name == "CStringSlice" || name.ends_with("$CStringSlice")
}

const fn is_integer_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::UInt | Ty::Bool | Ty::IntLiteral)
        || matches!(ty, Ty::Simd { width: 1, dtype } if is_integer_dtype(*dtype))
}

const fn is_integer_dtype(dtype: Dtype) -> bool {
    !dtype.is_float() && !matches!(dtype, Dtype::Bool)
}

const fn is_byte(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Simd {
            dtype: Dtype::Int8 | Dtype::UInt8,
            width: 1
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callee_names_are_unique_and_identifier_safe() {
        for (index, row) in CALLEES.iter().enumerate() {
            assert!(
                row.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{}",
                row.name
            );
            assert!(
                !CALLEES[..index].iter().any(|other| other.name == row.name),
                "duplicate callee {}",
                row.name
            );
        }
    }

    #[test]
    fn open_is_the_only_variadic_row() {
        let variadic: Vec<_> = CALLEES.iter().filter(|row| row.variadic()).collect();
        assert_eq!(variadic.len(), 1);
        assert_eq!(variadic[0].name, "open");
        assert_eq!(variadic[0].arity(), (2, 3));
        assert_eq!(variadic[0].param(2), Some(CType::UInt));
        assert_eq!(variadic[0].param(3), None);
    }

    #[test]
    fn argument_and_return_acceptance() {
        let int32 = Ty::Simd {
            dtype: Dtype::Int32,
            width: 1,
        };
        let byte_ptr = Ty::Pointer {
            element: Box::new(Ty::Simd {
                dtype: Dtype::UInt8,
                width: 1,
            }),
            origin: crate::origin::PointerOrigin::Untracked { mutable: true },
        };
        assert!(accepts_arg(CType::Int, &int32));
        assert!(accepts_arg(CType::Int, &Ty::Int));
        assert!(!accepts_arg(CType::Int, &Ty::StringLiteral));
        assert!(accepts_arg(CType::ConstCharPtr, &byte_ptr));
        assert!(accepts_arg(
            CType::ConstCharPtr,
            &Ty::Struct("__module$std$ffi$CStringSlice".to_string(), Vec::new())
        ));
        assert!(!accepts_arg(CType::ConstCharPtr, &Ty::Int));
        assert!(accepts_ret(CType::Int, &int32));
        assert!(accepts_ret(CType::SizeT, &Ty::UInt));
        assert!(accepts_ret(CType::CharPtr, &byte_ptr));
        assert!(!accepts_ret(CType::IntPtr, &byte_ptr));
        assert!(!accepts_ret(
            CType::Int,
            &Ty::Pointer {
                element: Box::new(Ty::Int),
                origin: crate::origin::PointerOrigin::Untracked { mutable: false },
            }
        ));
    }
}
