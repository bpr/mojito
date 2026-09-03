pub use mojito_ast::{ast, call};
pub use mojito_checked::checked;
pub use mojito_common::{error, literal, token};
pub use mojito_symbol::symbol;
pub use mojito_types::{ct, origin, types};

pub mod analysis;
pub mod artifact;
pub mod backend;
pub mod checker;
pub mod compiler;
pub mod comptime;
mod explicit_destroy;
pub mod hir;
pub mod lexer;
pub mod mir;
pub mod module;
pub mod native;
pub mod parser;
pub mod runtime;

// Re-export commonly used types at the crate root for convenience
pub use analysis::{check_ownership, check_ownership_checked, check_ownership_program};
pub use artifact::{ArtifactRunError, run_artifact};
pub use ast::{
    Dtype, Expr, ImportName, ImportNames, InfixOp, Param, PrefixOp, SourceType, Stmt, Type,
    TypeParam,
};
pub use backend::{Backend, BackendKind};
pub use checked::{
    CheckedCapture, CheckedConst, CheckedDeclId, CheckedDeclKind, CheckedDeclaration, CheckedExpr,
    CheckedNodeId, CheckedProgram, EffectFacts, IterationMode, IterationProtocol,
    SemanticAdjustment, ValueCategory,
};
pub use checker::{Checker, check, check_program};
pub use compiler::{CompiledProgram, Compiler, CompilerError, Execution, validate_module_scope};
pub use comptime::{ComptimeError, elaborate};
pub use ct::CtValue;
pub use error::{LexError, OwnershipError, ParseError, TypeError};
pub use lexer::Lexer;
pub use literal::{FloatLiteral, IntLiteral};
pub use module::{
    LinkOptions, ModuleError, inject_prelude, inject_prelude_with_options, link, link_source,
    link_source_with_options, link_with_options,
};
pub use origin::{
    CallableEnvironment, CaptureAccess, CaptureOrigin, CaptureOriginSet, CaptureSetParamId,
    Mutability, Origin, OriginParamId, OriginPlace, OriginSeg, OwnerId, PointerOrigin, RefTy,
};
pub use parser::{ParseReport, Parser};
pub use runtime::RuntimeError;
pub use runtime::Value;
pub use token::{SourceSpan, SyntaxId, Token};
pub use types::{ParamDecl, Ty, TyArg};

/// Lex `source` into its full token stream (a convenience for the **lex-only**
/// use of mojito as a syntax-analysis tool). Stops at the first `LexError`.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).map(|r| r.map(|(t, _)| t)).collect()
}

/// Parse `source` into the program AST (the **parse-only** front end — no type
/// checking or evaluation). Surfaces lexer errors as `ParseError::LexerError`.
pub fn parse(source: &str) -> Result<Vec<Stmt>, ParseError> {
    Parser::new(Lexer::new(source)).parse_program()
}

/// Parse for human-facing diagnostics, collecting up to `max_errors`. A report
/// with errors contains a quarantined partial AST and is not suitable for later
/// compiler stages.
pub fn parse_diagnostics(source: &str, max_errors: usize) -> ParseReport {
    Parser::new(Lexer::new(source)).parse_program_diagnostic(max_errors.max(1))
}
