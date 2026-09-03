//! Native lowering above the MIR waist: whole-program monomorphization
//! (`native::mono`) and native symbol mangling (`native::mangle`), re-exporting
//! the backend-independent core (`mojito-native-core`) so `native::layout`,
//! `native::target`, and `native::rt_abi` remain one surface.

pub mod native;
