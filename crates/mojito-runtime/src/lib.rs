//! Mojito's native runtime: the small, independently versioned C-ABI library
//! that native backends link into produced objects and executables.
//!
//! Every exported symbol uses the reserved `mjrt_` prefix (outside the `mj_`
//! user-symbol mangle image) and is specified twice: mechanically in the
//! `mojito::native::rt_abi` contract table, and normatively in
//! `docs/native-abi.md`, which defines size, alignment, ownership,
//! nullability, allocation responsibility, and failure behavior per symbol.
//! Tests in the `mojito` crate check this crate's signatures and `#[repr(C)]`
//! layouts against that table; keep the three in lockstep and bump
//! [`ABI_VERSION`] on any exported-contract change.
//!
//! This crate must stay dependency-free. In particular it must never depend on
//! the `mojito` crate: the VM's internal `Value` representation is not part of
//! any native ABI.

use std::alloc::Layout;
use std::io::Write;
use std::sync::Mutex;

/// The runtime ABI version. Bump on any change to an exported symbol's
/// signature, semantics, or to a `#[repr(C)]` type's layout.
///
/// Version 2: allocations carry a `{size, align}` header (so [`mjrt_free`]
/// releases without a size), zero-size requests allocate (header-only, so
/// every allocation is freeable), [`mjrt_dealloc`] validates against the
/// header instead of being undefined on mismatch, and
/// [`mjrt_unhandled_error`] reports an uncaught raise.
///
/// Version 3: raising functions return tagged `{tag, ok, err}` outcomes
/// through their outcome out-pointer (generated-code rule; no new type
/// crosses this ABI), and [`mjrt_trace`] reports ordered lifecycle events
/// from trace-instrumented builds.
///
/// Version 4: [`mjrt_read_line`] reads one stdin line for the `input()`
/// builtin.
///
/// Version 5: zero-size [`mjrt_alloc`] requests return the aligned dangling
/// sentinel instead of a header-only allocation, and
/// [`mjrt_free`]/[`mjrt_dealloc`] treat sentinel addresses (below the lowest
/// mappable page) as no-ops — an abandoned zero-size allocation leaks
/// nothing.
///
/// Version 6: allocations are tracked by payload address in addition to the
/// hidden layout header, so generated
/// code can diagnose dangling, use-after-free, and double-free operations;
/// [`mjrt_pointer_status`] exposes that check. [`mjrt_abort`] preserves an
/// abort's dynamic message, and trap categories 7 through 13 cover abort,
/// pointer-lifetime, and `MaybeUninit` failures.
pub const ABI_VERSION: u32 = 6;

/// Trap categories understood by [`mjrt_trap`]. Values match the backend's
/// trap numbering; the process exit code is `64 + category`.
pub const TRAP_DIV_MOD_ZERO: u32 = 1;
pub const TRAP_POW_EXPONENT: u32 = 2;
pub const TRAP_ALLOC_FAILURE: u32 = 3;
pub const TRAP_STDOUT_FAILURE: u32 = 4;
pub const TRAP_UNHANDLED_ERROR: u32 = 5;
pub const TRAP_STDIN_FAILURE: u32 = 6;
pub const TRAP_ABORT: u32 = 7;
pub const TRAP_POINTER_DANGLING: u32 = 8;
pub const TRAP_POINTER_USE_AFTER_FREE: u32 = 9;
pub const TRAP_POINTER_DOUBLE_FREE: u32 = 10;
pub const TRAP_UNINIT_READ: u32 = 11;
pub const TRAP_UNINIT_TAKE: u32 = 12;
pub const TRAP_UNINIT_DESTROY: u32 = 13;

/// Tag values for tagged success/error outcomes.
pub const MJ_TAG_OK: u32 = 0;
pub const MJ_TAG_ERR: u32 = 1;

/// Lifecycle-event kinds understood by [`mjrt_trace`]. Values match the
/// contract table in `mojito::native::rt_abi`.
pub const TRACE_DROP: u32 = 1;
pub const TRACE_CONSUME: u32 = 2;
pub const TRACE_CLEANUP: u32 = 3;
pub const TRACE_RAISE: u32 = 4;
pub const TRACE_CATCH: u32 = 5;

/// A borrowed string-literal descriptor: `len` bytes of UTF-8 at `data`, not
/// NUL-terminated, never owned by the callee.
#[repr(C)]
pub struct MjStrDesc {
    pub data: *const u8,
    pub len: u64,
}

/// The nominal `String` representation: `size` initialized bytes of UTF-8 in a
/// `cap`-byte allocation obtained from [`mjrt_alloc`].
#[repr(C)]
pub struct MjString {
    pub data: *mut u8,
    pub size: i64,
    pub cap: i64,
}

/// An error value carried on the tagged error path.
#[repr(C)]
pub struct MjError {
    pub message: MjString,
}

/// Inspectable ABI version of a linked runtime, exported as a data symbol so
/// tools (and, from Stage 3 on, startup checks) can read it without calling
/// anything.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static mjrt_abi_version: u32 = ABI_VERSION;

#[derive(Clone, Copy)]
struct AllocationRecord {
    size: usize,
    align: usize,
    live: bool,
}

const MAX_TRACKED_ALLOCATIONS: usize = 16_384;

fn allocations() -> &'static Mutex<[Option<(usize, AllocationRecord)>; MAX_TRACKED_ALLOCATIONS]> {
    // Keep bookkeeping out of the process heap that generated code uses.
    // This also makes lifetime checks allocation-free and deterministic.
    static ALLOCATIONS: Mutex<[Option<(usize, AllocationRecord)>; MAX_TRACKED_ALLOCATIONS]> =
        Mutex::new([None; MAX_TRACKED_ALLOCATIONS]);
    &ALLOCATIONS
}

/// Returns [`ABI_VERSION`].
#[unsafe(no_mangle)]
pub extern "C" fn mjrt_version() -> u32 {
    ABI_VERSION
}

/// Allocates `size` bytes aligned to `align`, prefixed by a hidden layout
/// header, and records it in the allocation registry. Never
/// returns null: allocation failure traps with [`TRAP_ALLOC_FAILURE`]. A
/// zero-size request allocates nothing and returns the aligned dangling
/// sentinel (`align` as an address — the same "aligned, never null" family
/// as dangling ZST pointers); [`mjrt_free`]/[`mjrt_dealloc`] recognize
/// sentinels as no-ops, so every returned pointer is freeable and an
/// abandoned zero-size allocation leaks nothing.
///
/// # Safety
///
/// `align` must be a nonzero power of two.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_alloc(size: u64, align: u64) -> *mut u8 {
    let (Ok(size), Ok(align)) = (usize::try_from(size), usize::try_from(align)) else {
        trap(TRAP_ALLOC_FAILURE)
    };
    if !align.is_power_of_two() {
        trap(TRAP_ALLOC_FAILURE)
    }
    if size == 0 {
        return align as *mut u8;
    }
    let header = align.max(16);
    let Some(total) = header.checked_add(size) else {
        trap(TRAP_ALLOC_FAILURE)
    };
    let Ok(layout) = Layout::from_size_align(total, header) else {
        trap(TRAP_ALLOC_FAILURE)
    };
    let base = unsafe { std::alloc::alloc(layout) };
    if base.is_null() {
        trap(TRAP_ALLOC_FAILURE)
    }
    let ptr = unsafe { base.add(header) };
    unsafe {
        (ptr.sub(16) as *mut u64).write(size as u64);
        (ptr.sub(8) as *mut u64).write(align as u64);
    }
    let mut records = allocations().lock().expect("allocation registry lock");
    // Reuse the existing identity before taking an empty slot. Allocators may
    // recycle an address after a valid free; leaving the old dead record in
    // an earlier slot would make pointer_status misclassify the new live
    // allocation as use-after-free.
    let index = records
        .iter()
        .position(|slot| slot.is_some_and(|(base, _)| base == ptr as usize))
        .or_else(|| records.iter().position(Option::is_none))
        .unwrap_or_else(|| trap(TRAP_ALLOC_FAILURE));
    let slot = &mut records[index];
    *slot = Some((
        ptr as usize,
        AllocationRecord {
            size,
            align,
            live: true,
        },
    ));
    if std::env::var_os("MOJITO_RT_DBG_POINTERS").is_some() {
        eprintln!("mjrt alloc {ptr:p} size={size} align={align}");
    }
    ptr
}

/// Whether `ptr` is a null or aligned-dangling sentinel rather than a real
/// allocation: sentinel addresses sit below the lowest mappable page
/// (`mmap_min_addr`, 65536 on the supported Linux target), where no
/// [`mjrt_alloc`] result can live.
fn is_dangling_sentinel(ptr: *const u8) -> bool {
    (ptr as usize) < 65536
}

/// Releases any allocation obtained from [`mjrt_alloc`] using its header —
/// the size-less free the language's `Pointer.unsafe_free()` family lowers
/// to. A null pointer or a zero-size aligned-dangling sentinel is a no-op.
///
/// # Safety
///
/// A non-sentinel `ptr` must come from [`mjrt_alloc`] and must not be used
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_free(ptr: *mut u8) {
    if is_dangling_sentinel(ptr) {
        return;
    }
    let record = {
        let mut records = allocations().lock().expect("allocation registry lock");
        let Some((_, record)) = records
            .iter_mut()
            .flatten()
            .find(|(base, _)| *base == ptr as usize)
        else {
            trap(TRAP_POINTER_DOUBLE_FREE)
        };
        if !record.live {
            trap(TRAP_POINTER_DOUBLE_FREE)
        }
        record.live = false;
        *record
    };
    if std::env::var_os("MOJITO_RT_DBG_POINTERS").is_some() {
        eprintln!("mjrt free {ptr:p} size={}", record.size);
    }
    unsafe { release(ptr, record.size, record.align) }
}

/// Classifies a pointer dereference: 0 live/ordinary, 1 dangling sentinel,
/// 2 an address inside a freed Mojito allocation.
#[unsafe(no_mangle)]
pub extern "C" fn mjrt_pointer_status(ptr: *const u8) -> u32 {
    if is_dangling_sentinel(ptr) {
        return 1;
    }
    let address = ptr as usize;
    let records = allocations().lock().expect("allocation registry lock");
    let contains = |base: usize, record: &AllocationRecord| {
        let extent = record.size.max(1);
        address >= base && address < base.saturating_add(extent)
    };
    if records
        .iter()
        .flatten()
        .any(|(base, record)| record.live && contains(*base, record))
    {
        return 0;
    }
    if let Some((base, record)) = records
        .iter()
        .flatten()
        .find(|(base, record)| !record.live && contains(*base, record))
    {
        if std::env::var_os("MOJITO_RT_DBG_POINTERS").is_some() {
            eprintln!(
                "mjrt stale {ptr:p} in base={:#x} size={}",
                *base, record.size
            );
        }
        return 2;
    }
    0
}

/// Releases an allocation obtained from [`mjrt_alloc`], validating the
/// caller's `size` and `align` against the allocation header — a mismatch
/// traps with [`TRAP_ALLOC_FAILURE`] instead of corrupting the heap. A null
/// pointer or a zero-size aligned-dangling sentinel is a no-op.
///
/// # Safety
///
/// A non-sentinel `ptr` must come from [`mjrt_alloc`] and must not be used
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_dealloc(ptr: *mut u8, size: u64, align: u64) {
    if is_dangling_sentinel(ptr) {
        return;
    }
    let record = {
        let mut records = allocations().lock().expect("allocation registry lock");
        let Some((_, record)) = records
            .iter_mut()
            .flatten()
            .find(|(base, _)| *base == ptr as usize)
        else {
            trap(TRAP_POINTER_DOUBLE_FREE)
        };
        if !record.live {
            trap(TRAP_POINTER_DOUBLE_FREE)
        }
        if record.size != size as usize || record.align != align as usize {
            trap(TRAP_ALLOC_FAILURE)
        }
        record.live = false;
        *record
    };
    unsafe { release(ptr, record.size, record.align) }
}

/// Writes exactly `len` bytes to stdout (retrying on interruption) and
/// flushes. A zero `len` is a no-op and ignores `data`. A write failure traps
/// with [`TRAP_STDOUT_FAILURE`].
///
/// # Safety
///
/// When `len` is nonzero, `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_write_stdout(data: *const u8, len: u64) {
    if len == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
    let mut out = std::io::stdout().lock();
    if out.write_all(bytes).and_then(|()| out.flush()).is_err() {
        trap(TRAP_STDOUT_FAILURE)
    }
}

/// Formats `value` in decimal into `out` and returns the byte length written.
/// `out` must hold at least 20 bytes (fits `i64::MIN`). No NUL terminator.
///
/// # Safety
///
/// `out` must point to at least 20 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_fmt_i64(value: i64, out: *mut u8) -> u64 {
    unsafe { copy_out(&value.to_string(), out) }
}

/// Formats `value` in decimal into `out` and returns the byte length written.
/// `out` must hold at least 20 bytes (fits `u64::MAX`). No NUL terminator.
///
/// # Safety
///
/// `out` must point to at least 20 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_fmt_u64(value: u64, out: *mut u8) -> u64 {
    unsafe { copy_out(&value.to_string(), out) }
}

/// Formats `value` as the VM displays `Float64` — Rust's `{:?}`: the shortest
/// text that round-trips, always keeping a decimal point or exponent (`3.0`,
/// `1e300`, `NaN`, `inf`) — into `out` and returns the byte length written.
/// `out` must hold at least 32 bytes. No NUL terminator.
///
/// # Safety
///
/// `out` must point to at least 32 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_fmt_f64(value: f64, out: *mut u8) -> u64 {
    unsafe { copy_out(&format!("{value:?}"), out) }
}

/// Reports the trap `category` on stderr and exits the process with
/// `64 + category` (categories above 63 clamp so the code stays in exit-code
/// range). Never returns; runs no destructors.
#[unsafe(no_mangle)]
pub extern "C" fn mjrt_trap(category: u32) -> ! {
    trap(category)
}

/// Reports an uncaught raised error — `unhandled error: <message>` on
/// stderr — and exits with the [`TRAP_UNHANDLED_ERROR`] exit code. `data` is
/// the borrowed UTF-8 message. Never returns; runs no destructors.
///
/// # Safety
///
/// When `len` is nonzero, `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_unhandled_error(data: *const u8, len: u64) -> ! {
    let message = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let _ = writeln!(
        std::io::stderr(),
        "unhandled error: {}",
        String::from_utf8_lossy(message)
    );
    std::process::exit(trap_exit_code(TRAP_UNHANDLED_ERROR))
}

/// Reports an uncatchable language abort with its dynamic message.
///
/// # Safety
///
/// When `len` is nonzero, `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_abort(data: *const u8, len: u64) -> ! {
    let message = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let _ = writeln!(
        std::io::stderr(),
        "abort: {}",
        String::from_utf8_lossy(message)
    );
    std::process::exit(trap_exit_code(TRAP_ABORT))
}

/// Reports one ordered lifecycle event — `mjtrace <kind> <payload>` (or
/// `mjtrace <kind>` for an empty payload) on stderr — from a
/// trace-instrumented build. Default emission never calls this; stdout byte
/// parity is untouched because the trace goes to stderr. Write errors are
/// ignored: tracing must never perturb observable program behavior.
///
/// # Safety
///
/// When `len` is nonzero, `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_trace(kind: u32, data: *const u8, len: u64) {
    let payload = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let name = trace_kind_name(kind);
    let mut err = std::io::stderr().lock();
    let _ = if payload.is_empty() {
        writeln!(err, "mjtrace {name}")
    } else {
        writeln!(err, "mjtrace {name} {}", String::from_utf8_lossy(payload))
    };
}

/// Reads one line from stdin for the `input()` builtin, writing an
/// [`MjString`] into `out`: `data` is a fresh [`mjrt_alloc`] allocation the
/// caller owns, with `size == cap ==` the line length after stripping the
/// trailing `\n` (then `\r`). EOF yields size 0 with a valid header-only
/// allocation, so every result is uniformly freeable and noninteractive runs
/// never block. A read error traps with [`TRAP_STDIN_FAILURE`].
///
/// # Safety
///
/// `out` must point to at least 24 writable bytes (one [`MjString`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mjrt_read_line(out: *mut u8) {
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        trap(TRAP_STDIN_FAILURE)
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    let len = line.len();
    let data = unsafe { mjrt_alloc(len as u64, 1) };
    unsafe {
        std::ptr::copy_nonoverlapping(line.as_ptr(), data, len);
        out.cast::<MjString>().write_unaligned(MjString {
            data,
            size: len as i64,
            cap: len as i64,
        });
    }
}

/// The stable stderr name of a lifecycle-event kind.
pub fn trace_kind_name(kind: u32) -> &'static str {
    match kind {
        TRACE_DROP => "drop",
        TRACE_CONSUME => "consume",
        TRACE_CLEANUP => "cleanup",
        TRACE_RAISE => "raise",
        TRACE_CATCH => "catch",
        _ => "event",
    }
}

/// The stderr text for a trap category. Known categories reuse the VM's
/// runtime-error message text so the two backends diagnose identically.
pub fn trap_message(category: u32) -> &'static str {
    match category {
        TRAP_DIV_MOD_ZERO => "integer division or modulo by zero",
        TRAP_POW_EXPONENT => "'**' exponent must be a non-negative Int that fits in 32 bits",
        TRAP_ALLOC_FAILURE => "allocation failed",
        TRAP_STDOUT_FAILURE => "stdout write failed",
        TRAP_UNHANDLED_ERROR => "unhandled error",
        TRAP_STDIN_FAILURE => "stdin read failed",
        TRAP_ABORT => "abort",
        TRAP_POINTER_DANGLING => "vm: dereference of dangling Pointer",
        TRAP_POINTER_USE_AFTER_FREE => "vm: use after Pointer deallocation",
        TRAP_POINTER_DOUBLE_FREE => "vm: double free of Pointer allocation",
        TRAP_UNINIT_READ => "vm: read of uninitialized MaybeUninit storage",
        TRAP_UNINIT_TAKE => "vm: take of uninitialized MaybeUninit storage",
        TRAP_UNINIT_DESTROY => "vm: destroy of uninitialized MaybeUninit storage",
        _ => "unknown trap",
    }
}

/// The process exit code for a trap category: `64 + category`, clamped to 127.
pub fn trap_exit_code(category: u32) -> i32 {
    64 + category.min(63) as i32
}

fn trap(category: u32) -> ! {
    let _ = writeln!(
        std::io::stderr(),
        "mojito runtime trap: {}",
        trap_message(category)
    );
    std::process::exit(trap_exit_code(category))
}

/// Releases a headered allocation with its registry-validated layout.
unsafe fn release(ptr: *mut u8, size: usize, align: usize) {
    let header = align.max(16);
    let layout = Layout::from_size_align(header + size, header)
        .expect("a live allocation's layout was valid at mjrt_alloc time");
    unsafe { std::alloc::dealloc(ptr.sub(header), layout) };
}

/// Copies `text` into `out`, returning its byte length. Callers guarantee the
/// buffer contracts above; the widest possible texts are 20 bytes for
/// `i64`/`u64` and 24 bytes for the `f64` `{:?}` form.
unsafe fn copy_out(text: &str, out: *mut u8) -> u64 {
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) };
    text.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt_i64(value: i64) -> String {
        let mut buf = [0u8; 20];
        let len = unsafe { mjrt_fmt_i64(value, buf.as_mut_ptr()) } as usize;
        String::from_utf8(buf[..len].to_vec()).unwrap()
    }

    fn fmt_u64(value: u64) -> String {
        let mut buf = [0u8; 20];
        let len = unsafe { mjrt_fmt_u64(value, buf.as_mut_ptr()) } as usize;
        String::from_utf8(buf[..len].to_vec()).unwrap()
    }

    fn fmt_f64(value: f64) -> String {
        let mut buf = [0u8; 32];
        let len = unsafe { mjrt_fmt_f64(value, buf.as_mut_ptr()) } as usize;
        String::from_utf8(buf[..len].to_vec()).unwrap()
    }

    #[test]
    fn version_symbols_agree() {
        assert_eq!(mjrt_version(), ABI_VERSION);
        assert_eq!(mjrt_abi_version, ABI_VERSION);
    }

    #[test]
    fn alloc_dealloc_round_trip() {
        let ptr = unsafe { mjrt_alloc(64, 8) };
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 8, 0);
        unsafe {
            ptr.write_bytes(0xAB, 64);
            assert_eq!(*ptr, 0xAB);
            mjrt_dealloc(ptr, 64, 8);
        }
    }

    #[test]
    fn allocation_registry_supports_sizeless_free() {
        let ptr = unsafe { mjrt_alloc(48, 32) };
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 32, 0);
        unsafe {
            ptr.write_bytes(0xCD, 48);
            mjrt_free(ptr);
        }
    }

    #[test]
    fn zero_size_alloc_is_the_freeable_dangling_sentinel() {
        let ptr = unsafe { mjrt_alloc(0, 16) };
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize, 16);
        unsafe { mjrt_free(ptr) };
        let again = unsafe { mjrt_alloc(0, 8) };
        assert_eq!(again as usize, 8);
        unsafe { mjrt_dealloc(again, 0, 8) };
    }

    #[test]
    fn free_and_dealloc_ignore_null() {
        unsafe { mjrt_free(std::ptr::null_mut()) };
        unsafe { mjrt_dealloc(std::ptr::null_mut(), 64, 8) };
    }

    #[test]
    fn fmt_matches_vm_display_family() {
        assert_eq!(fmt_i64(0), "0");
        assert_eq!(fmt_i64(i64::MIN), i64::MIN.to_string());
        assert_eq!(fmt_i64(i64::MAX), i64::MAX.to_string());
        assert_eq!(fmt_u64(u64::MAX), u64::MAX.to_string());
        // The VM displays Float64 with `{:?}` (runtime.rs); the widest such
        // text must fit the documented 32-byte buffer.
        for x in [
            0.0,
            -0.0,
            3.0,
            2.5,
            1e300,
            -1.7976931348623157e308,
            -2.2250738585072014e-308,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let expected = format!("{x:?}");
            assert!(expected.len() <= 32);
            assert_eq!(fmt_f64(x), expected);
        }
    }

    #[test]
    fn write_stdout_zero_len_ignores_data() {
        unsafe { mjrt_write_stdout(std::ptr::null(), 0) };
    }

    #[test]
    fn trace_kind_names_are_stable() {
        assert_eq!(trace_kind_name(TRACE_DROP), "drop");
        assert_eq!(trace_kind_name(TRACE_CONSUME), "consume");
        assert_eq!(trace_kind_name(TRACE_CLEANUP), "cleanup");
        assert_eq!(trace_kind_name(TRACE_RAISE), "raise");
        assert_eq!(trace_kind_name(TRACE_CATCH), "catch");
        assert_eq!(trace_kind_name(0), "event");
        // A zero-length payload ignores data entirely.
        unsafe { mjrt_trace(TRACE_RAISE, std::ptr::null(), 0) };
    }

    #[test]
    fn trap_codes_and_messages() {
        assert_eq!(trap_exit_code(TRAP_DIV_MOD_ZERO), 65);
        assert_eq!(trap_exit_code(TRAP_POW_EXPONENT), 66);
        assert_eq!(trap_exit_code(TRAP_ALLOC_FAILURE), 67);
        assert_eq!(trap_exit_code(TRAP_STDOUT_FAILURE), 68);
        assert_eq!(trap_exit_code(TRAP_STDIN_FAILURE), 70);
        assert_eq!(trap_exit_code(TRAP_ABORT), 71);
        assert_eq!(trap_exit_code(TRAP_POINTER_DANGLING), 72);
        assert_eq!(trap_exit_code(TRAP_POINTER_USE_AFTER_FREE), 73);
        assert_eq!(trap_exit_code(TRAP_POINTER_DOUBLE_FREE), 74);
        assert_eq!(trap_exit_code(TRAP_UNINIT_READ), 75);
        assert_eq!(trap_exit_code(TRAP_UNINIT_TAKE), 76);
        assert_eq!(trap_exit_code(TRAP_UNINIT_DESTROY), 77);
        assert_eq!(trap_exit_code(u32::MAX), 64 + 63);
        assert_eq!(
            trap_message(TRAP_DIV_MOD_ZERO),
            "integer division or modulo by zero"
        );
        assert_eq!(trap_message(999), "unknown trap");
    }
}
