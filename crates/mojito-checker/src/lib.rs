//! Semantic checking: name/type/origin checking over the linked, elaborated
//! AST, producing the `CheckedProgram` handoff. Includes the explicit-destroy
//! deletability analysis (`explicit_destroy`).

pub mod checker;
pub mod explicit_destroy;
