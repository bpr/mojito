//! The generated native capability matrix (roadmap: native backend, Stage 5).
//!
//! Renders `conformance/pliron-capability.tsv`: one row per textual-MIR
//! instruction mnemonic, one per checked-type constructor spelling, and one
//! per exported runtime symbol. The instruction and type tables are
//! maintained here and mechanically pinned against the canonical schema
//! vocabulary ([`mojito_mir::mir::text::INSTRUCTION_MNEMONICS`],
//! [`mojito_mir::mir::text::TYPE_SPELLINGS`]) — a new MIR instruction or type
//! constructor fails the pin until it receives an explicit capability
//! decision — while the runtime section derives directly from the `rt_abi`
//! contract tables, `since` versions included.

use mojito_native::native::rt_abi::{MJRT_ABI_VERSION, RT_DATA_SYMBOLS, RT_SYMBOLS};

/// Render the complete capability matrix in its canonical TSV form.
pub fn matrix() -> String {
    let mut out = String::from(
        "# Pliron native capability matrix (generated; schema-version 1).\n\
         # One row per capability subject:\n\
         #   section <TAB> subject <TAB> status <TAB> note\n\
         # section: instruction (textual-MIR mnemonic) |\n\
         #          type (checked-type constructor spelling) |\n\
         #          runtime (exported mojito-runtime symbol)\n\
         # status: supported | partial (the note states the lowered condition) |\n\
         #         unsupported (contextual rejection; the note names the gap)\n\
         # Regenerate: UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD \\\n\
         #   cargo nextest run --features backend-pliron capability_matrix\n",
    );
    for (subject, status, note) in INSTR_CAPABILITIES {
        push_row(&mut out, "instruction", subject, *status, note);
    }
    for (subject, status, note) in TYPE_CAPABILITIES {
        push_row(&mut out, "type", subject, *status, note);
    }
    for sig in RT_SYMBOLS {
        push_row(
            &mut out,
            "runtime",
            sig.symbol,
            CapabilityStatus::Supported,
            &format!("since=v{}", sig.since),
        );
    }
    for (symbol, _) in RT_DATA_SYMBOLS {
        push_row(
            &mut out,
            "runtime",
            symbol,
            CapabilityStatus::Supported,
            &format!("data symbol; current version {MJRT_ABI_VERSION}"),
        );
    }
    out
}

/// How completely the backend lowers one subject of the matrix.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    Supported,
    Partial,
    Unsupported,
}

impl CapabilityStatus {
    pub fn spelling(self) -> &'static str {
        match self {
            CapabilityStatus::Supported => "supported",
            CapabilityStatus::Partial => "partial",
            CapabilityStatus::Unsupported => "unsupported",
        }
    }
}

/// Per-instruction capability, in [`mojito_mir::mir::text::INSTRUCTION_MNEMONICS`]
/// order (pinned by the module tests). `partial` rows state the lowered
/// condition; a
/// `supported` row may still reject a malformed artifact (untyped registers,
/// missing blocks) — those are verifier-level anomalies, not capability gaps.
pub const INSTR_CAPABILITIES: &[(&str, CapabilityStatus, &str)] = &[
    (
        "loans.establish",
        CapabilityStatus::Supported,
        "ownership fact; erased after validation",
    ),
    (
        "interiors.invalidate",
        CapabilityStatus::Supported,
        "ownership fact; erased after validation",
    ),
    (
        "ref.make",
        CapabilityStatus::Supported,
        "materializes a verified place address",
    ),
    (
        "ref.read",
        CapabilityStatus::Partial,
        "typed referent registers",
    ),
    (
        "value.copy",
        CapabilityStatus::Partial,
        "compiled `__copyinit__` only; raising or uncompiled copy constructors reject",
    ),
    (
        "ref.write",
        CapabilityStatus::Partial,
        "typed referent registers",
    ),
    (
        "closure.make",
        CapabilityStatus::Partial,
        "two-word `{invoke, env}` values over per-target thunks with frame-local capture records; owned droppable captures drop through the tombstoning record header; projected move captures and owned captures with a user copy/move constructor reject",
    ),
    ("lifetime.keep_alive", CapabilityStatus::Supported, "erased"),
    (
        "const",
        CapabilityStatus::Partial,
        "Int/Float/Bool/Str/None (literals stay pending until materialization) and function references as two-word callable values",
    ),
    (
        "literal.materialize",
        CapabilityStatus::Partial,
        "scalar and width-1 SIMD targets (wrapping, VM-exact) plus exact literal-typed storage; a constant exceeding i64/f64 storage rejects",
    ),
    ("var.use", CapabilityStatus::Supported, "copy/move/borrow"),
    (
        "place.move",
        CapabilityStatus::Partial,
        "marks the root partially initialized; later whole-variable drops of it reject",
    ),
    (
        "var.store",
        CapabilityStatus::Supported,
        "sets the initialization flag",
    ),
    (
        "unary",
        CapabilityStatus::Partial,
        "checked scalar operand types",
    ),
    (
        "binary",
        CapabilityStatus::Partial,
        "checked scalar operand types plus pointer `+` element arithmetic; nominal left operands monomorphize to operator-method calls; other forms reject",
    ),
    (
        "call",
        CapabilityStatus::Partial,
        "positional and keyword arguments (including `mut`/`ref` places) with constant defaults, plus the `len`/`abs`/`min`/`max`/`round`/`divmod`/`input`/`_mojito_abort` builtins, conversion-dunder rewrites, and the `UnsafePointer` allocation family; value param-args reject",
    ),
    (
        "call.indirect",
        CapabilityStatus::Partial,
        "contract-bound positional and keyword arguments through `{invoke, env}` values, raising contracts included; variadic/keyword-variadic contracts, defaulted arguments, reference returns, generic callable values, and un-devirtualized nominal callables reject",
    ),
    (
        "call.method",
        CapabilityStatus::Partial,
        "positional and keyword contracts on compiled targets, the scalar `__floor__`/`__ceil__`/`__trunc__`/`__ceildiv__` intrinsics, the `Writer.write`/`write_to` display dispatches, and the String struct-to-literal bridge; reference-result adapters reject",
    ),
    (
        "pointer.take",
        CapabilityStatus::Partial,
        "element-offset raw move out of ownership-verified single-take storage; unlayoutable elements reject",
    ),
    (
        "pointer.destroy",
        CapabilityStatus::Partial,
        "in-place element destructor at the element offset; raising or droppable-field destructors reject",
    ),
    (
        "uninit.make",
        CapabilityStatus::Partial,
        "payload-only frame storage, no init flag (verified programs never take or destroy uninitialized slots)",
    ),
    (
        "uninit.take",
        CapabilityStatus::Partial,
        "raw byte move of the payload",
    ),
    (
        "uninit.destroy",
        CapabilityStatus::Partial,
        "in-place payload destructor; raising or droppable-field destructors reject",
    ),
    (
        "field.get",
        CapabilityStatus::Partial,
        "layoutable base aggregates",
    ),
    (
        "index.get",
        CapabilityStatus::Partial,
        "nominal `__getitem__` dispatch (value, reference, and raising-reference results) plus the `pointer` and constant-index pack-storage intrinsics; runtime pack indexes reject until the packs slice",
    ),
    (
        "slice.get",
        CapabilityStatus::Supported,
        "raw bound descriptors through the nominal `__getitem__` contract; bound accesses materialize compiled `Optional`s, `indices` normalizes natively (two-element for `ContiguousSlice`), explicit `Slice(...)`/`slice(...)` construction builds the descriptor, `Slice.__eq__`/`__ne__` compare the raw words, and printing writes `Slice(start, end, step)`",
    ),
    (
        "index.multi",
        CapabilityStatus::Supported,
        "variadic `__getitem__` dispatch with index and slice-descriptor actuals, positional or keyword",
    ),
    (
        "index.multi_set",
        CapabilityStatus::Supported,
        "`__setitem__` dispatch with the keyword or trailing-positional value slot and `mut self` write-back",
    ),
    (
        "place.store",
        CapabilityStatus::Supported,
        "whole-variable stores set the initialization flag",
    ),
    (
        "place.store_ref",
        CapabilityStatus::Supported,
        "stores the reference handle",
    ),
    (
        "place.load",
        CapabilityStatus::Partial,
        "layoutable, typed places",
    ),
    (
        "tuple.make",
        CapabilityStatus::Supported,
        "typed element lists; variadic call sites build packs by relocation",
    ),
    (
        "variant.make",
        CapabilityStatus::Supported,
        "tagged aggregate construction over the normative Variant layout",
    ),
    (
        "variant.is",
        CapabilityStatus::Supported,
        "checked tag comparison",
    ),
    (
        "variant.get",
        CapabilityStatus::Supported,
        "checked payload projection",
    ),
    (
        "variant.set",
        CapabilityStatus::Supported,
        "drop-active-payload then replace through a checked place",
    ),
    (
        "variant.take",
        CapabilityStatus::Supported,
        "checked and unchecked consuming payload extraction",
    ),
    (
        "variant.set_init_with",
        CapabilityStatus::Supported,
        "indirect factory invocation followed by owning replacement",
    ),
    (
        "variant.deinit_with",
        CapabilityStatus::Supported,
        "tag-checked consuming handler invocation",
    ),
    (
        "variant.replace",
        CapabilityStatus::Supported,
        "owning replacement returning the previous payload",
    ),
    (
        "simd.make",
        CapabilityStatus::Supported,
        "width-1 scalar aliases and multi-lane scalar aggregates with VM-exact lane conversion",
    ),
    (
        "simd.cast",
        CapabilityStatus::Partial,
        "element-wise int rewrap, f64-mediated float conversion, and i128-saturating float-to-int; bool casts reject in the checker",
    ),
    (
        "simd.shuffle",
        CapabilityStatus::Supported,
        "compile-time lane gather over scalar-aggregate storage",
    ),
    (
        "raise",
        CapabilityStatus::Supported,
        "tagged-outcome error edge",
    ),
    (
        "try",
        CapabilityStatus::Supported,
        "structural flattening with cleanup drops and finally dispatch",
    ),
    (
        "drop.reg",
        CapabilityStatus::Unsupported,
        "reserved for the future assembler VM; the VM rejects it too",
    ),
    (
        "drop.var",
        CapabilityStatus::Partial,
        "flag-guarded compiled destructors, including destructor-bearing structs with droppable fields; depth-1 partial moves drop surviving leaves under per-leaf presence flags (any tombstone suppresses `__deinit__`); deeper moves reject",
    ),
    (
        "consume.var",
        CapabilityStatus::Supported,
        "clears the initialization flag; surviving droppable struct fields destroy in reverse order under the per-leaf flags",
    ),
    (
        "consume.place",
        CapabilityStatus::Partial,
        "no droppable residual fields",
    ),
    (
        "unsupported",
        CapabilityStatus::Unsupported,
        "explicit lowering marker; rejects by definition",
    ),
    (
        "iter.init",
        CapabilityStatus::Partial,
        "concrete post-mono `__iter__` chains: aliased `ref`/`mut`, byte-copied `read`, and consuming `var` receivers; raising `__iter__` steps and stepless heap-owning splits reject",
    ),
    (
        "iter.has_next",
        CapabilityStatus::Partial,
        "bounded nominal `__len__() > 0`; the compiler-private pack fallback rejects",
    ),
    (
        "iter.next",
        CapabilityStatus::Partial,
        "nominal `__next__(mut self)` aliasing the iterator slot: value results, reference results, and the `CopyIteratorReference` adapter; the pack fallback rejects",
    ),
    (
        "iter.try_next",
        CapabilityStatus::Partial,
        "tagged-outcome `__next__(mut self)` with the statically typed StopIteration edge, including reference-yielding raising iterators (copy and `for ref` contracts); elements with user destructors reject",
    ),
];

/// Per-type-constructor capability, in [`mojito_mir::mir::text::TYPE_SPELLINGS`]
/// order (pinned by the module tests).
pub const TYPE_CAPABILITIES: &[(&str, CapabilityStatus, &str)] = &[
    ("Int", CapabilityStatus::Supported, "signless i64"),
    (
        "UInt",
        CapabilityStatus::Supported,
        "signless i64; unsigned operator selection",
    ),
    ("Bool", CapabilityStatus::Supported, "i1"),
    (
        "StringLiteral",
        CapabilityStatus::Supported,
        "constant-pool literal registers and typed storage (borrowed MjStrDesc descriptor)",
    ),
    ("Float64", CapabilityStatus::Supported, "f64"),
    ("None", CapabilityStatus::Supported, "zero-sized"),
    (
        "Never",
        CapabilityStatus::Unsupported,
        "uninhabited; no lowering",
    ),
    (
        "IntLiteral",
        CapabilityStatus::Partial,
        "pending-literal registers and exact i64 typed storage; a constant exceeding i64 rejects (the VM keeps arbitrary precision)",
    ),
    (
        "FloatLiteral",
        CapabilityStatus::Partial,
        "pending-literal registers and f64 typed storage; displaying a runtime value rejects (the VM prints the exact rational)",
    ),
    (
        "Infer",
        CapabilityStatus::Unsupported,
        "checker-internal; must not reach verified MIR",
    ),
    ("DType", CapabilityStatus::Unsupported, "compile-time only"),
    (
        "func",
        CapabilityStatus::Partial,
        "two-word `{invoke, env}` value; the environment record is frame-local",
    ),
    (
        "generic_func",
        CapabilityStatus::Unsupported,
        "semantic-only; Stage 5 monomorphization target",
    ),
    ("overload", CapabilityStatus::Unsupported, "semantic-only"),
    (
        "param",
        CapabilityStatus::Unsupported,
        "monomorphization substitutes receiver-bound instances; a residual parameter is a contextual rejection",
    ),
    (
        "assoc",
        CapabilityStatus::Unsupported,
        "semantic-only; Stage 5 monomorphization target",
    ),
    (
        "dependent_index",
        CapabilityStatus::Unsupported,
        "semantic-only; Stage 5 monomorphization target",
    ),
    ("Self", CapabilityStatus::Unsupported, "semantic-only"),
    (
        "struct_type",
        CapabilityStatus::Partial,
        "concrete field layouts only",
    ),
    (
        "simd",
        CapabilityStatus::Supported,
        "width-1 scalar aliases plus multi-lane scalar aggregates with element-wise arithmetic, conversion, indexing, shuffle, reductions, selection, and formatting",
    ),
    ("Error", CapabilityStatus::Supported, "MjError storage"),
    (
        "comptime_list",
        CapabilityStatus::Unsupported,
        "CTFE bridge; no runtime values",
    ),
    (
        "tuple",
        CapabilityStatus::Partial,
        "concrete element layouts only",
    ),
    (
        "runtime_pack",
        CapabilityStatus::Partial,
        "concrete element layouts only",
    ),
    (
        "variadic_pack",
        CapabilityStatus::Partial,
        "monomorphization reifies call-site arities into concrete runtime packs; unspecialized packs never reach lowering",
    ),
    (
        "variant",
        CapabilityStatus::Supported,
        "normative tag/payload layout with dynamic active-payload destruction",
    ),
    (
        "pointer",
        CapabilityStatus::Supported,
        "one opaque pointer; element layouts checked at use sites",
    ),
    (
        "ref",
        CapabilityStatus::Supported,
        "one opaque place address",
    ),
];

fn push_row(out: &mut String, section: &str, subject: &str, status: CapabilityStatus, note: &str) {
    out.push_str(&format!(
        "{section}\t{subject}\t{status}\t{note}\n",
        status = status.spelling()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_mir::mir::text::{INSTRUCTION_MNEMONICS, TYPE_SPELLINGS};

    /// A new MIR instruction must receive an explicit capability decision.
    #[test]
    fn pliron_instr_capabilities_cover_the_instruction_vocabulary() {
        let subjects: Vec<&str> = INSTR_CAPABILITIES
            .iter()
            .map(|(subject, _, _)| *subject)
            .collect();
        assert_eq!(
            subjects, INSTRUCTION_MNEMONICS,
            "instruction capability rows must match the canonical mnemonic \
             inventory exactly (same members, same order)"
        );
    }

    /// A new checked-type constructor must receive an explicit capability
    /// decision.
    #[test]
    fn pliron_type_capabilities_cover_the_type_vocabulary() {
        let subjects: Vec<&str> = TYPE_CAPABILITIES
            .iter()
            .map(|(subject, _, _)| *subject)
            .collect();
        assert_eq!(
            subjects, TYPE_SPELLINGS,
            "type capability rows must match the canonical spelling \
             inventory exactly (same members, same order)"
        );
    }
}
