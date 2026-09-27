//! Surface syntax: the AST node vocabulary, the structural call-binding
//! policy (`call`) that owns argument-to-parameter matching, and the
//! lane-width scan (`simd_width`) the elaborator and source validation share.
//! Depends only on `mojito-common`.

pub mod ast;
pub mod call;
pub mod simd_width;
pub mod visit;
