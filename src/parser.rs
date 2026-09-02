//! Hand-written parser for Mojito's indentation-sensitive Mojo subset.
//!
//! Statement, declaration, type, and suite parsing use recursive descent;
//! expressions use precedence climbing with postfix call/member/index tails.
//! [`parse`](crate::parse) is fail-fast, while the diagnostic entry point
//! recovers at statement boundaries so it can report multiple syntax errors.

use std::iter::Peekable;

use crate::ast::{
    ArgConvention, Capture, CaptureKind, CaptureList, Decorator, Expr, ExprKind, FnParam,
    FunctionTypeParam, InfixOp, KwArg, LoopBindingMode, Method, Param, ParamKind, PrefixOp, Stmt,
    StmtKind, SubscriptArg, TStringPart, Type, WithItem,
};
use crate::error::{LexError, ParseError};
use crate::lexer::Lexer;
use crate::token::{Span, TStringChunk, Token};

/// A syntax-only recovery result. `program` is deliberately partial and must not
/// be sent to semantic phases when `errors` is non-empty.
#[derive(Debug)]
pub struct ParseReport {
    pub program: Vec<Stmt>,
    pub errors: Vec<ParseError>,
    pub truncated: bool,
}

mod driver;
mod exprs;
mod items;
mod stmts;
mod suffix;
mod types;

/// Binding-power levels, lowest to highest. Mirrors Python/Mojo expression
/// precedence for the implemented operator set (no bitwise / `**` yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Precedence {
    Lowest,
    Walrus,      // name := value  (binds looser than everything else)
    Conditional, // a if c else b  (ternary; looser than `or`, tighter than walrus)
    Or,          // or
    And,         // and
    Not,         // not x  (prefix)
    Comparison,  // == != < > <= >=
    Sum,         // + -
    Product,     // * / // %
    Unary,       // -x  (prefix)
    Power,       // **  (right-associative, binds tighter than unary -)
    Call,        // f(...)  .field  .method(...)
}

enum ParsedBracketItem {
    Param(crate::ast::ParamArg),
    Slice {
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
        explicit_step: bool,
    },
    KeywordSlice {
        name: String,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
        explicit_step: bool,
    },
}

/// A parsed parameter list: the parameters plus the positions of the `/`
/// (positional-only) and bare `*` (keyword-only) markers, if present.
struct ParamList {
    params: Vec<FnParam>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
}

/// Maps a contextual convention word (`imm`/`mut`/`out`/`ref`/`deinit`) to
/// its `ArgConvention`, or `None` for any other identifier. The removed
/// `read` spelling is not a convention; convention positions reject it with
/// [`removed_convention_error`].
fn convention_word(word: &str) -> Option<ArgConvention> {
    match word {
        "imm" => Some(ArgConvention::Imm),
        "mut" => Some(ArgConvention::Mut),
        "out" => Some(ArgConvention::Out),
        "ref" => Some(ArgConvention::Ref),
        "deinit" => Some(ArgConvention::Deinit),
        _ => None,
    }
}

/// The targeted migration diagnostic for the removed `read` convention
/// (upstream hard error since 2026-08: `'read' was removed; use 'imm'`), or
/// `None` when the word is not the removed spelling.
fn removed_convention_error(word: &str) -> Option<ParseError> {
    (word == "read").then(|| {
        ParseError::UnexpectedToken(
            Token::Identifier(word.to_string()),
            "'read' was removed; use 'imm'".to_string(),
        )
    })
}

/// The callee name of a call: the callee must be a bare identifier (closures
/// can't escape to become arbitrary callee expressions).
fn call_name(callee: Expr) -> Result<String, ParseError> {
    match callee.kind {
        ExprKind::Identifier(name) => Ok(name),
        other => Err(ParseError::UnexpectedToken(
            Token::LParen,
            format!("only named functions can be called, found {:?}", other),
        )),
    }
}

/// Whether a value-shaped bracket argument is unambiguously an Origin
/// specialization rather than a runtime subscript. Most compile-time values
/// cannot be distinguished syntactically from indices; `origin_of(...)` can,
/// because it is a compiler-known operation that never has a runtime value.
fn is_explicit_origin_argument(expression: &Expr) -> bool {
    matches!(
        &expression.kind,
        ExprKind::Call {
            name,
            param_args,
            kwargs,
            args,
        } if name == "origin_of"
            && param_args.is_empty()
            && kwargs.is_empty()
            && !args.is_empty()
    )
}

/// The parsed body of an `if`/`elif`/`else` chain: the `(condition, body)`
/// branches plus the optional `else` body. Shared by `if` and `comptime if`.
type IfChain = (Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>);

type StructConformanceList = (Vec<String>, Vec<(String, Expr)>, Option<Type>);

type ParsedSliceTail = (Option<Box<Expr>>, Option<Box<Expr>>, bool);

/// Parses a t-string interpolation's raw source as a single expression, on a
/// fresh sub-lexer/parser. The whole fragment must be one expression (trailing
/// tokens are an error).
fn parse_interpolation(src: &str) -> Result<Expr, ParseError> {
    let mut sub = Parser::new(Lexer::new(src));
    let expr = sub.parse_expression(Precedence::Lowest)?;
    sub.expect_stmt_end()?; // reject leftover tokens (e.g. `{a b}`)
    Ok(expr)
}

/// Append a literal component while coalescing neighboring text. Keeping the
/// normalized t-string tree compact also gives MIR one constant per run rather
/// than one per source token.
fn push_tstring_literal(parts: &mut Vec<TStringPart>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(TStringPart::Literal(previous)) = parts.last_mut() {
        previous.push_str(&text);
    } else {
        parts.push(TStringPart::Literal(text));
    }
}

fn param_argument_name(arg: &crate::ast::ParamArg) -> Result<String, ParseError> {
    match arg {
        crate::ast::ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => Ok(name.clone()),
        _ => Err(ParseError::UnexpectedToken(
            Token::Assign,
            "a compile-time keyword argument requires a name".into(),
        )),
    }
}

fn expression_name_starts_lowercase(expression: &Expr) -> bool {
    matches!(
        &expression.kind,
        ExprKind::Identifier(name)
            if name.chars().next().is_some_and(|character| character.is_lowercase())
    )
}

/// The infix operator an augmented-assignment token applies (`+=` → `Add`, …),
/// or `None` if the token is not an augmented-assignment operator.
fn aug_assign_op(tok: &Token) -> Option<InfixOp> {
    Some(match tok {
        Token::PlusEq => InfixOp::Add,
        Token::MinusEq => InfixOp::Sub,
        Token::StarEq => InfixOp::Mul,
        Token::SlashEq => InfixOp::Div,
        Token::DoubleSlashEq => InfixOp::FloorDiv,
        Token::PercentEq => InfixOp::Mod,
        Token::DoubleStarEq => InfixOp::Pow,
        Token::AmpEq => InfixOp::BitAnd,
        Token::PipeEq => InfixOp::BitOr,
        Token::CaretEq => InfixOp::BitXor,
        _ => return None,
    })
}

/// The precedence an infix operator parses its right operand at (left-assoc).
fn infix_precedence(op: InfixOp) -> Precedence {
    match op {
        InfixOp::Or => Precedence::Or,
        InfixOp::And => Precedence::And,
        InfixOp::Eq
        | InfixOp::Ne
        | InfixOp::Lt
        | InfixOp::Gt
        | InfixOp::Le
        | InfixOp::Ge
        | InfixOp::In
        | InfixOp::NotIn => Precedence::Comparison,
        InfixOp::Add
        | InfixOp::Sub
        | InfixOp::Shl
        | InfixOp::Shr
        | InfixOp::BitAnd
        | InfixOp::BitOr
        | InfixOp::BitXor => Precedence::Sum,
        InfixOp::Mul | InfixOp::Div | InfixOp::FloorDiv | InfixOp::Mod | InfixOp::MatMul => {
            Precedence::Product
        }
        // Right-associative: parse the right operand one level below `**` so that
        // a following `**` (Power > Unary) is re-absorbed (`a ** b ** c` = `a ** (b ** c)`).
        InfixOp::Pow => Precedence::Unary,
    }
}

// --- Recursive Descent + Pratt Parser ---

pub struct Parser<I: Iterator<Item = Result<(Token, Span), LexError>>> {
    tokens: Peekable<I>,
    /// Span of the most recently consumed token. Together with a `start` mark
    /// captured before a node begins, this yields each AST node's span (a node
    /// spans from its first token's start to its last token's end — see `node`).
    last_span: Span,
    /// End offset of the last *significant* token — i.e. excluding the layout
    /// tokens (`Newline`/`Indent`/`Dedent`/`Eof`). Used for statement spans, so a
    /// statement doesn't swallow the trailing newline `expect_stmt_end` consumes.
    last_significant_end: usize,
    /// Whether the most recent statement terminator was `;` rather than a newline
    /// or EOF. Used for one-line suites like `def f(): a(); b()`.
    last_stmt_ended_with_semicolon: bool,
}

/// Whether a type is the qualified struct binder (`Self.o`) or a projection
/// chain rooted at it whose applications are single value arguments —
/// re-materializable as an origin expression by
/// `Parser::rematerialize_self_param_chain`.
fn self_param_rooted_expression(ty: &Type) -> Option<()> {
    match ty {
        Type::SelfParam(_) => Some(()),
        Type::Assoc { base, args, .. } => {
            self_param_rooted_expression(base)?;
            match args.as_slice() {
                [] | [crate::ast::ParamArg::Value(_)] => Some(()),
                _ => None,
            }
        }
        Type::IndexedProjection { base, .. } => self_param_rooted_expression(base),
        _ => None,
    }
}
