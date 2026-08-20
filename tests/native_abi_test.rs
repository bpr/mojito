//! Rust-side mechanical agreement between the `mojito-runtime` crate and the
//! shared native ABI contract (`mojito::native::rt_abi`), plus the layout
//! owner's pinned representations. Runs in the default (LLVM-free) lane; the
//! LLVM-side halves of these checks live in `tests/pliron_backend_test.rs`.

use std::mem::{align_of, offset_of, size_of};

use mojito::native::layout::{Layout, LayoutCx, StructFieldIndex};
use mojito::native::rt_abi::{self, CAbiTy};
use mojito::native::target::{NativeTarget, Triple};
use mojito::types::Ty;

/// Maps a Rust parameter/result type onto its contract-table primitive.
trait HasCAbi {
    const ABI: CAbiTy;
}

impl HasCAbi for u32 {
    const ABI: CAbiTy = CAbiTy::U32;
}
impl HasCAbi for u64 {
    const ABI: CAbiTy = CAbiTy::U64;
}
impl HasCAbi for i64 {
    const ABI: CAbiTy = CAbiTy::I64;
}
impl HasCAbi for f64 {
    const ABI: CAbiTy = CAbiTy::F64;
}
impl HasCAbi for *const u8 {
    const ABI: CAbiTy = CAbiTy::PtrConstU8;
}
impl HasCAbi for *mut u8 {
    const ABI: CAbiTy = CAbiTy::PtrMutU8;
}

/// Maps a Rust return type onto the table's `Option<CAbiTy>` form.
trait HasCAbiRet {
    const ABI_RET: Option<CAbiTy>;
}

impl HasCAbiRet for () {
    const ABI_RET: Option<CAbiTy> = None;
}
impl<T: HasCAbi> HasCAbiRet for T {
    const ABI_RET: Option<CAbiTy> = Some(T::ABI);
}

fn row(symbol: &str) -> &'static rt_abi::RtSig {
    rt_abi::find_symbol(symbol)
        .unwrap_or_else(|| panic!("runtime exports '{symbol}' but RT_SYMBOLS has no row for it"))
}

/// Checks one exported function against its contract row twice over: the
/// coercion to the spelled-out `extern "C"` fn-pointer type pins the actual
/// Rust signature, and the `HasCAbi` projection of those same spellings pins
/// the table row.
macro_rules! assert_sig {
    ($symbol:ident: fn($($param:ty),*) -> $ret:ty) => {{
        let _: unsafe extern "C" fn($($param),*) -> $ret = mojito_runtime::$symbol;
        let row = row(stringify!($symbol));
        let expected: Vec<CAbiTy> = vec![$(<$param as HasCAbi>::ABI),*];
        let actual: Vec<CAbiTy> = row.params.iter().map(|(_, ty)| *ty).collect();
        assert_eq!(actual, expected, "{} parameter kinds", row.symbol);
        assert_eq!(row.ret, <$ret as HasCAbiRet>::ABI_RET, "{} return kind", row.symbol);
        assert!(!row.noreturn, "{} is not noreturn", row.symbol);
    }};
}

/// Checks one exported `#[repr(C)]` type against its `RT_TYPES` layout on the
/// x86_64 target (pointer sizes agree because this test only runs on 64-bit
/// hosts).
macro_rules! assert_type_layout {
    ($name:ident { $($field:ident),+ }) => {{
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        let spec = rt_abi::find_type(stringify!($name))
            .unwrap_or_else(|| panic!("RT_TYPES has no entry for {}", stringify!($name)));
        let layout = rt_abi::type_layout(spec, &target);
        assert_eq!(
            (layout.layout.size, layout.layout.align),
            (
                size_of::<mojito_runtime::$name>() as u64,
                align_of::<mojito_runtime::$name>() as u64
            ),
            "{} size/align",
            stringify!($name)
        );
        let declared: Vec<&str> = spec.fields.iter().map(|field| field.name).collect();
        assert_eq!(declared, vec![$(stringify!($field)),+], "{} field order", stringify!($name));
        let offsets = vec![$(offset_of!(mojito_runtime::$name, $field) as u64),+];
        assert_eq!(layout.offsets, offsets, "{} field offsets", stringify!($name));
    }};
}

#[cfg(target_pointer_width = "64")]
#[test]
fn runtime_signatures_match_the_contract_table() {
    assert_sig!(mjrt_version: fn() -> u32);
    assert_sig!(mjrt_alloc: fn(u64, u64) -> *mut u8);
    assert_sig!(mjrt_free: fn(*mut u8) -> ());
    assert_sig!(mjrt_dealloc: fn(*mut u8, u64, u64) -> ());
    assert_sig!(mjrt_write_stdout: fn(*const u8, u64) -> ());
    assert_sig!(mjrt_fmt_i64: fn(i64, *mut u8) -> u64);
    assert_sig!(mjrt_fmt_u64: fn(u64, *mut u8) -> u64);
    assert_sig!(mjrt_fmt_f64: fn(f64, *mut u8) -> u64);
    // `-> !` cannot go through the generic return projection on stable Rust;
    // pin the noreturn contracts by hand.
    let _: unsafe extern "C" fn(u32) -> ! = mojito_runtime::mjrt_trap;
    let trap = row("mjrt_trap");
    assert!(trap.noreturn && trap.ret.is_none());
    assert_eq!(
        trap.params.iter().map(|(_, ty)| *ty).collect::<Vec<_>>(),
        vec![CAbiTy::U32]
    );
    let _: unsafe extern "C" fn(*const u8, u64) -> ! = mojito_runtime::mjrt_unhandled_error;
    let unhandled = row("mjrt_unhandled_error");
    assert!(unhandled.noreturn && unhandled.ret.is_none());
    assert_eq!(
        unhandled
            .params
            .iter()
            .map(|(_, ty)| *ty)
            .collect::<Vec<_>>(),
        vec![CAbiTy::PtrConstU8, CAbiTy::U64]
    );
}

#[test]
fn every_table_row_has_exactly_one_check_above() {
    // Adding a runtime symbol must extend runtime_signatures_match_the
    // _contract_table; this count trips when the table grows without it.
    assert_eq!(rt_abi::RT_SYMBOLS.len(), 10);
    assert_eq!(rt_abi::RT_DATA_SYMBOLS.len(), 1);
    assert_eq!(rt_abi::RT_TYPES.len(), 3);
}

#[cfg(target_pointer_width = "64")]
#[test]
fn runtime_type_layouts_match_the_contract_table() {
    assert_type_layout!(MjStrDesc { data, len });
    assert_type_layout!(MjString { data, size, cap });
    assert_type_layout!(MjError { message });
}

#[test]
fn abi_version_and_shared_constants_agree() {
    assert_eq!(mojito_runtime::ABI_VERSION, rt_abi::MJRT_ABI_VERSION);
    assert_eq!(mojito_runtime::TRAP_DIV_MOD_ZERO, rt_abi::TRAP_DIV_MOD_ZERO);
    assert_eq!(mojito_runtime::TRAP_POW_EXPONENT, rt_abi::TRAP_POW_EXPONENT);
    assert_eq!(
        mojito_runtime::TRAP_ALLOC_FAILURE,
        rt_abi::TRAP_ALLOC_FAILURE
    );
    assert_eq!(
        mojito_runtime::TRAP_STDOUT_FAILURE,
        rt_abi::TRAP_STDOUT_FAILURE
    );
    assert_eq!(
        mojito_runtime::TRAP_UNHANDLED_ERROR,
        rt_abi::TRAP_UNHANDLED_ERROR
    );
    assert_eq!(mojito_runtime::MJ_TAG_OK, rt_abi::MJ_TAG_OK);
    assert_eq!(mojito_runtime::MJ_TAG_ERR, rt_abi::MJ_TAG_ERR);
    // The inspectable data symbol carries the same version the table pins.
    assert_eq!(mojito_runtime::mjrt_abi_version, rt_abi::MJRT_ABI_VERSION);
}

#[cfg(target_pointer_width = "64")]
#[test]
fn checked_types_with_specified_runtime_representations_agree() {
    let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
    let structs = StructFieldIndex::default();
    let cx = LayoutCx {
        target: &target,
        structs: &structs,
    };
    // StringLiteral lowers to the MjStrDesc descriptor.
    assert_eq!(
        cx.layout_of(&Ty::StringLiteral),
        Ok(Layout::new(
            size_of::<mojito_runtime::MjStrDesc>() as u64,
            align_of::<mojito_runtime::MjStrDesc>() as u64
        ))
    );
    // The built-in Error value is MjError.
    assert_eq!(
        cx.layout_of(&Ty::Error),
        Ok(Layout::new(
            size_of::<mojito_runtime::MjError>() as u64,
            align_of::<mojito_runtime::MjError>() as u64
        ))
    );
    // The nominal String struct ({ptr, Int, Int} fields) is MjString.
    let mut structs = StructFieldIndex::default();
    structs.insert(
        "String".to_string(),
        vec![
            Ty::Pointer {
                element: Box::new(Ty::UInt),
                origin: mojito::origin::PointerOrigin::Static,
            },
            Ty::Int,
            Ty::Int,
        ],
    );
    let cx = LayoutCx {
        target: &target,
        structs: &structs,
    };
    assert_eq!(
        cx.layout_of(&Ty::Struct("String".to_string(), vec![])),
        Ok(Layout::new(
            size_of::<mojito_runtime::MjString>() as u64,
            align_of::<mojito_runtime::MjString>() as u64
        ))
    );
}
