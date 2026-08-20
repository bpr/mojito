//! Deterministic C-safe mangling of MIR symbol names.
//!
//! MIR function names are already globally unique and deterministic
//! (`src/symbol.rs` owns callable identity), but they contain characters that
//! are not valid in native symbols (`.`, `$`, `;`, `[`, `]`, …). This module
//! applies a purely mechanical, injective escape; it never re-derives
//! identity.
//!
//! Scheme: prefix `mj_`; then per byte, `[A-Za-z0-9]` passes through, `_`
//! becomes `_u`, and every other byte becomes `_hh` (two lowercase hex
//! digits). Injective because every `_` in the output starts an escape and
//! `u` is not a hex digit; the `mj_` prefix keeps mangled names clear of the
//! synthesized executable wrapper `main`, the C `exit`, and the reserved
//! `mjrt_*` runtime namespace — the `mojito-runtime` crate's exports and
//! backend-emitted helpers (e.g. `mjrt_pow`) alike.

/// Mangle a MIR symbol name into a C-safe native symbol.
pub fn mangle(name: &str) -> String {
    let mut out = String::with_capacity(3 + name.len());
    out.push_str("mj_");
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(byte as char),
            b'_' => out.push_str("_u"),
            other => out.push_str(&format!("_{other:02x}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mangle;

    /// Inverse of [mangle], for round-trip tests only.
    fn demangle(symbol: &str) -> Option<String> {
        let rest = symbol.strip_prefix("mj_")?;
        let bytes = rest.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'_' => {
                    if bytes.get(i + 1) == Some(&b'u') {
                        out.push(b'_');
                        i += 2;
                    } else {
                        let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                        out.push(u8::from_str_radix(hex, 16).ok()?);
                        i += 3;
                    }
                }
                other => {
                    out.push(other);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).ok()
    }

    fn is_c_safe(symbol: &str) -> bool {
        !symbol.is_empty()
            && !symbol.starts_with(|c: char| c.is_ascii_digit())
            && symbol
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    #[test]
    fn simple_names_stay_readable() {
        assert_eq!(mangle("fib"), "mj_fib");
        assert_eq!(mangle("main"), "mj_main");
        assert_eq!(mangle("__toplevel__"), "mj__u_utoplevel_u_u");
    }

    #[test]
    fn full_overload_names_are_c_safe_and_round_trip() {
        let names = [
            "Array.__init__$ov$T$AnyType$kw$fill",
            "Array.__iter__$ov$SelfRef",
            "__module$std$range$_SequentialRange$dint;",
            "Box.__init__$ov$Int",
            "pick$ov$Pair$Int",
        ];
        for name in names {
            let mangled = mangle(name);
            assert!(is_c_safe(&mangled), "{mangled}");
            assert_eq!(demangle(&mangled).as_deref(), Some(name));
        }
    }

    #[test]
    fn mangling_is_injective_on_confusable_pairs() {
        // Pairs crafted so a naive escape could collide.
        let pairs = [
            ("a_b", "a.b"),
            ("a_ub", "a_b"),
            ("a$b", "a.b"),
            ("_", "u"),
            ("a24", "a$"),
        ];
        for (left, right) in pairs {
            assert_ne!(mangle(left), mangle(right), "{left} vs {right}");
        }
    }

    #[test]
    fn wrapper_symbol_never_collides() {
        // The exe wrapper is named `main`; every mangled symbol has the
        // `mj_` prefix, so no MIR name can shadow it.
        assert_ne!(mangle("main"), "main");
        assert!(mangle("main").starts_with("mj_"));
    }

    #[test]
    fn runtime_helper_symbols_never_collide() {
        // `exit` and the `mjrt_*` helpers are declared unmangled by the
        // lowering. Every mangled name starts with exactly `mj_` (third byte
        // `_`), so `mjrt_pow` (third byte `r`) and prefix-less `exit` are
        // outside the mangle image no matter the source name.
        for source in ["exit", "mjrt_pow", "rt_pow", "t_pow"] {
            let mangled = mangle(source);
            assert!(mangled.starts_with("mj_"), "{mangled}");
            assert_ne!(mangled, "mjrt_pow");
            assert_ne!(mangled, "exit");
        }
    }
}
