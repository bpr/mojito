//! Compile-time elaboration: `comptime` blocks, specialization, and CTFE.
//! Sits ABOVE the VM in the crate DAG even though elaboration runs before
//! checking in the pipeline — CTFE compiles and executes sub-programs
//! through `VmBackend`.

pub mod comptime;
