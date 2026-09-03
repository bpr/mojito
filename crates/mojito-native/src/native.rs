//! The shared native target, layout, and runtime-ABI owner.
//!
//! Every native backend (Pliron today, Cranelift or others later) consumes
//! this module instead of inventing its own target model, data layout, symbol
//! mangling, or runtime contract; the normative specification is
//! `docs/native-abi.md`. The module is pure Rust with no LLVM dependency, so
//! the default (VM-only) build compiles and tests it; backends validate their
//! emitted IR against it behind their own feature gates.
//!
//! Naming note: the root crate's `runtime` module is the VM's internal value layer. Nothing in
//! this module or in the `mojito-runtime` crate may depend on that
//! representation — the native ABI is defined only by the types here.

pub mod mangle;
pub mod mono;

pub use mojito_native_core::{layout, rt_abi, target};
