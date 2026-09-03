//! The shared native target, layout, and runtime-ABI owner — the
//! backend-independent half of `src/native`. Every native backend consumes
//! these instead of inventing its own target model, data layout, or runtime
//! contract; the normative specification is `docs/native-abi.md`. Pure Rust
//! with no LLVM dependency, sitting BELOW the MIR waist (the MIR verifier's
//! layout probe uses it), while monomorphization and mangling remain above
//! in the root `native` module.

pub mod layout;
pub mod rt_abi;
pub mod target;
