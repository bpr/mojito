//! Error taxonomy for each compiler boundary.
//!
//! Lexing, parsing, module linking, compile-time elaboration, semantic checking,
//! ownership analysis, and execution retain distinct error types so the driver
//! can report the stage that rejected a program without flattening diagnostics
//! into unstructured strings.

use crate::token::Token;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    IndentationError(usize),
    UnmatchedParenthesis(usize),
    UnexpectedCharacter(char, usize),
    InvalidInteger(usize),
    InvalidFloat(usize),
    UnterminatedString(usize),
    UnterminatedIdentifier(usize),
    InvalidEscape(char, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    LexerError(LexError),
    UnexpectedToken(Token, String),
    /// A complete upstream-shaped sentence, rendered verbatim.
    Message(String),
    UnexpectedEof(String),
    UnknownType(String),
    At {
        err: Box<Self>,
        span: crate::token::Span,
    },
}

impl LexError {
    pub const fn byte_pos(&self) -> usize {
        match self {
            Self::IndentationError(pos)
            | Self::UnmatchedParenthesis(pos)
            | Self::UnexpectedCharacter(_, pos)
            | Self::InvalidInteger(pos)
            | Self::InvalidFloat(pos)
            | Self::UnterminatedString(pos)
            | Self::UnterminatedIdentifier(pos)
            | Self::InvalidEscape(_, pos) => *pos,
        }
    }
}

impl ParseError {
    #[must_use]
    pub fn at(self, span: crate::token::Span) -> Self {
        if self.byte_pos().is_some() {
            self
        } else {
            Self::At {
                err: Box::new(self),
                span,
            }
        }
    }

    pub const fn byte_pos(&self) -> Option<usize> {
        match self {
            Self::LexerError(err) => Some(err.byte_pos()),
            Self::At { span, .. } => Some(span.0),
            Self::UnexpectedToken(_, _)
            | Self::Message(_)
            | Self::UnexpectedEof(_)
            | Self::UnknownType(_) => None,
        }
    }
}

/// Errors from semantic checking, produced before HIR/MIR lowering. These cover
/// type, declaration, call, trait, convention, and locally decidable borrow rules.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeError {
    UndefinedVariable(String),
    /// A storage annotation (a struct field or local `var` type) applied a
    /// generic whose explicit origin parameters were left entirely unbound.
    NotConcrete(String),
    /// A storage annotation omitted an explicit parameter the position cannot
    /// infer (origin slots infer only from constructor value arguments).
    CannotInferParam {
        name: String,
        param: String,
    },
    /// An executable statement appeared at module scope. Mojo modules contain
    /// declarations and compile-time constants; runtime work belongs in a
    /// function such as `main`.
    InvalidModuleScope(String),
    /// A raising operation appeared outside a `raises` function or protected
    /// `try` body.
    UnhandledRaise(String),
    /// A `with` statement's manager does not satisfy the context-manager
    /// protocol; the text is upstream's diagnostic verbatim.
    ContextManager(String),
    RaiseTypeMismatch {
        expected: String,
        found: String,
    },
    /// A non-`Copyable` value is used where it would be copied (bound to a new
    /// variable, passed by value, returned, …). Mojo move-only semantics: transfer
    /// it with `^`, or make the type `Copyable`. `ty` is the type; `context` is the
    /// site (e.g. `variable 'b'`).
    NonCopyable {
        ty: String,
        context: String,
    },
    /// A Copyable-but-not-ImplicitlyCopyable place was used in an owned-value
    /// position without an explicit transfer or copy.
    ImplicitCopy {
        ty: String,
        context: String,
        transferable: bool,
        copyable: bool,
    },
    /// Aliasing violation: a variable is borrowed mutably (`mut`/`ref`) and also
    /// borrowed (mutably or shared) at the same call — Mojo's borrow rule is
    /// mutable-XOR-shared. E.g. `f(mut a, mut a)` or `f(mut a, a)`.
    AliasingViolation {
        var: String,
    },
    ExplicitDestroy {
        var: String,
        message: String,
        problem: String,
    },
    /// A name used in call position is not function-typed.
    NotCallable {
        name: String,
        ty: String,
    },
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
    /// Re-declaring a name already bound in the same scope.
    Redeclaration(String),
    /// A free or nested function uses a word reserved for future Mojo syntax.
    /// These words remain lexer-level identifiers so later syntax can adopt
    /// them contextually.
    ReservedName(String),
    /// A bare assignment (`x = e` or `a, b = e`) targets a name that is not in
    /// scope. Mojito requires `var` to declare a new variable.
    AssignToUndeclared(String),
    /// Assignment attempted through an immutable binding, such as an ordinary
    /// function parameter. Mojo function arguments are immutable unless their
    /// convention makes them writable (`mut`, `ref`, `out`).
    ImmutableBinding(String),
    /// An augmented assignment whose destination binding is immutable.
    ImmutableInPlaceDestination(String),
    /// A reference return is rooted in storage not named by its declared origin.
    ReturnsReferenceToLocal,
    /// A store into storage that outlives the frame carries a loan rooted in
    /// frame-local storage — the store-outward twin of the return escape.
    StoredReferenceEscapesOrigin,
    /// The two-phase transfer-effect pass kept observing stale callee
    /// effects after the round cap — effects should grow monotonically to a
    /// small fixpoint, so this indicates a checker defect rather than a
    /// user error.
    TransferEffectDivergence {
        rounds: usize,
        callable: String,
    },
    /// An origin-bearing pointer would outlive the checked storage it
    /// designates, e.g. returning `UnsafePointer(to=local)`.
    PointerEscapesOrigin,
    /// A function value tried to escape by being returned (downward funargs
    /// only). The static counterpart of `RuntimeError::ClosureEscape`.
    ClosureEscape,
    /// `return` appearing outside any function body.
    ReturnOutsideFunction,
    /// `break` appearing outside any loop.
    BreakOutsideLoop,
    /// `continue` appearing outside any loop.
    ContinueOutsideLoop,
    /// An inferred type did not match the one required by its context (a
    /// variable annotation, a declared return type, or a parameter type).
    TypeMismatch {
        expected: String,
        found: String,
        context: String,
    },
    /// An operator applied to operand type(s) it is not defined for.
    BadOperator {
        op: String,
        operands: String,
    },
    /// An augmented assignment (`OP=`) on a user-defined value whose type does
    /// not define the required in-place dunder (`+=` needs `__iadd__`, …). Mojo
    /// dispatches augmented assignment to the dedicated in-place method and does
    /// not fall back to the ordinary binary operator.
    MissingInPlaceOperator {
        op: String,
        ty: String,
    },
    /// A type annotation named an identifier that is not a known type/struct.
    UnknownType(String),
    /// Field access on a value whose type has no such field.
    NoSuchField {
        object_type: String,
        field: String,
    },
    /// Associated type/comptime member lookup in type position failed.
    NoSuchAssociatedType {
        object_type: String,
        member: String,
    },
    /// Method call on a value whose type has no such method.
    NoSuchMethod {
        object_type: String,
        method: String,
    },
    /// Constructing a struct that has no constructor (no `@fieldwise_init`).
    NoConstructor(String),
    /// An `out self` lifecycle method (`__init__`/`__copyinit__`/`__moveinit__`)
    /// leaves a declared field unassigned (definite initialization: every field must
    /// be initialized in the body).
    UninitializedField {
        struct_name: String,
        method: String,
        field: String,
    },
    /// A struct declares both `@fieldwise_init` and a hand-written `__init__`
    /// (each defines a constructor — the decorator *generates* `__init__`).
    ConflictingConstructor(String),
    /// A type-parameter bound named a trait that is not a recognized built-in
    /// (user-defined traits are not supported yet).
    UnknownTrait(String),
    /// A leading-dot contextual member reference (`.red`) could not resolve
    /// its base type: no contextual type was available, or the expected type
    /// cannot supply members (non-struct or generic).
    ContextualMember {
        member: String,
        reason: String,
    },
    /// A parameterized type was applied to the wrong number of type arguments
    /// (e.g. `Pair[Int, Int]` for a one-parameter `Pair`, or type arguments on a
    /// non-generic type).
    WrongTypeArgCount {
        name: String,
        expected: usize,
        got: usize,
    },
    /// `Self.T` used where `T` is not a type parameter of the enclosing struct
    /// (or outside any struct).
    UnknownSelfParam(String),
    /// The enclosing struct's own parameter spelled bare (`T`, `n`) inside
    /// its body, where Mojo requires `Self.T` / `Self.n`.
    UnqualifiedStructParam(String),
    /// `Self.field` in a compile-time position (a bracket slot, an alias),
    /// where only a parameter or an associated member is in scope.
    InstanceFieldWithoutInstance {
        field: String,
        ty: String,
    },
    /// A field typed by a bare struct type parameter whose bounds do not
    /// prove `Deinitable`, in a struct that declares no `Deinitable`
    /// conformance to discharge it.
    FieldNotDeinitable {
        field: String,
        ty: String,
    },
    /// A generic call/construction could not solve a type parameter from the
    /// argument types (no explicit type-argument syntax exists to supply it).
    CannotInferTypeParam {
        name: String,
        param: String,
    },
    /// A solved type argument does not conform to a type parameter's declared
    /// trait bound (`f[T: Quackable](...)` called with a non-`Quackable` type).
    TraitNotSatisfied {
        param: String,
        ty: String,
        trait_name: String,
        /// The first concrete missing field or operation, when the checker can
        /// identify one without obscuring the primary bound failure.
        reason: Option<String>,
    },
    /// A struct declares conformance to a trait but is missing a required method.
    MissingTraitMethod {
        struct_name: String,
        trait_name: String,
        method: String,
    },
    /// A struct's method exists but does not match the trait's required signature.
    TraitMethodMismatch {
        struct_name: String,
        trait_name: String,
        method: String,
    },
    /// A struct declares conformance to a trait but is missing a required
    /// associated compile-time member.
    MissingTraitComptimeMember {
        struct_name: String,
        trait_name: String,
        member: String,
    },
    /// A struct's associated compile-time member exists but has the wrong
    /// compile-time kind or type for the trait requirement.
    TraitComptimeMemberMismatch {
        struct_name: String,
        trait_name: String,
        member: String,
    },
    /// A value parameter was declared with a type other than `Int` (the only
    /// value-parameter type supported).
    BadValueParamType {
        name: String,
        ty: String,
    },
    /// An expression used in a compile-time position (`comptime NAME = …`, or a
    /// value-parameter argument) is not a constant `Int` expression.
    NotComptime(String),
    /// A `SIMD` element-type argument was not a recognized `DType.<name>`.
    BadDtype(String),
    /// A SIMD width was not a positive power of two.
    BadSimdWidth(String),
    /// A SIMD construction had the wrong number of element arguments (must be the
    /// width, or exactly one to splat).
    SimdArity {
        width: i64,
        got: usize,
    },
    /// A subscript `v[i]` was applied to a non-SIMD value.
    NotIndexable(String),
    /// A function with a non-`None` return type can fall off the end without
    /// returning (does not return on every path).
    MissingReturn(String),
    /// A mutating `List` method (`append`/`pop`) was called on something other
    /// than a plain list variable (mojito has no general member-write, so the
    /// receiver must be a variable whose list can be mutated in place).
    MutationRequiresVariable(String),
    /// A field of `self` was written (`self.x = e`) in a method whose receiver is
    /// a read-only `self`. Mutating `self` requires the `mut self` convention.
    ImmutableSelf,
    /// The left side of an assignment is not a valid place (a variable, or a
    /// field/index chain rooted at one).
    InvalidAssignTarget(String),
    /// A valid-Mojo construct that mojito **parses** (and the AST carries) but
    /// does not implement — flagged at check time because it can't be
    /// meaningfully type-checked (e.g. a `def` with `*args`, `**kwargs`, argument
    /// conventions, or `/`/`*` markers; a keyword-argument call to a method).
    /// Carries a message describing the feature. The runtime analogue is
    /// `RuntimeError::Unsupported`.
    Unsupported(String),
    /// A per-instantiation method clone of a generic struct failed to check
    /// with the parameters bound (Mojo's post-instantiation diagnostic): the
    /// instance, the source method, and the concrete error.
    PostInstantiation {
        receiver: String,
        method: String,
        error: Box<Self>,
    },
    /// A compiler phase received state that violates a contract established by
    /// an earlier phase. This is a Mojito bug, not an error in the source file.
    InvariantViolation(String),
    /// A call whose arguments don't match the callee's parameters in a way arity
    /// alone doesn't capture: an unknown keyword name, a parameter bound twice
    /// (positionally and by keyword, or a duplicate keyword), or a required
    /// parameter left unbound. `reason` describes the specific problem.
    BadCall {
        func: String,
        reason: String,
    },
    /// A value consumed inside a `try` body is used in its `except` arm: the
    /// consuming call may have raised after taking it.
    UninitializedUse {
        var: String,
    },
    /// No overload of a built-in callable accepts the argument (`len` on a
    /// value that is not `Sized`).
    NoMatchingFunction {
        name: String,
    },
    /// An `@explicit_destroy` value was abandoned — left at scope end,
    /// overwritten, or discarded — without reaching a named destructor.
    Abandoned {
        var: String,
        message: String,
    },
    /// A value typed by a type parameter whose bounds do not prove
    /// `Deinitable` reached the end of its scope without being explicitly
    /// destroyed (moved on or consumed).
    LinearAbandoned {
        var: String,
        message: String,
    },
    /// A `ref[<exact origin>]` parameter (`ImmStaticOrigin`,
    /// `ImmUntrackedOrigin`, `MutUntrackedOrigin`) received an actual whose
    /// origin is not that origin. The call site rewrites it into a `BadCall`
    /// naming the function and parameter.
    RefOriginMismatch {
        slot: usize,
        ty: String,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndentationError(pos) => write!(f, "Indentation error at byte {pos}"),
            Self::UnmatchedParenthesis(pos) => {
                write!(f, "Unmatched closing parenthesis at byte {pos}")
            }
            Self::UnexpectedCharacter(c, pos) => {
                write!(f, "Unexpected character '{c}' at byte {pos}")
            }
            Self::InvalidInteger(pos) => {
                write!(f, "Invalid integer literal starting at byte {pos}")
            }
            Self::InvalidFloat(pos) => {
                write!(f, "Invalid float literal starting at byte {pos}")
            }
            Self::UnterminatedString(pos) => {
                write!(f, "Unterminated string literal starting at byte {pos}")
            }
            Self::UnterminatedIdentifier(pos) => {
                write!(f, "Unterminated backtick identifier starting at byte {pos}")
            }
            Self::InvalidEscape(c, pos) => {
                write!(f, "Invalid string escape '\\{c}' at byte {pos}")
            }
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LexerError(err) => write!(f, "Lexer error: {err}"),
            Self::UnexpectedToken(token, msg) => {
                write!(f, "Unexpected token {token:?}: {msg}")
            }
            Self::Message(msg) => write!(f, "{msg}"),
            Self::UnexpectedEof(msg) => write!(f, "Unexpected EOF: {msg}"),
            Self::UnknownType(name) => write!(f, "Unknown type '{name}'"),
            Self::At { err, span } => write!(f, "{err} at byte {}", span.0),
        }
    }
}

impl fmt::Display for TypeError {
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedVariable(name) => write!(f, "Undefined variable '{name}'"),
            Self::NotConcrete(name) => write!(
                f,
                "'{name}[_]' is not concrete; use '[]' to bind missing parameters"
            ),
            Self::CannotInferParam { name, param } => write!(
                f,
                "'{name}' failed to infer parameter '{param}'; specify the parameter or use '_' or '...' to unbind the parameter explicitly"
            ),
            Self::InvalidModuleScope(statement) => write!(
                f,
                "{statement} is not allowed at file scope; move executable code into a function body"
            ),
            Self::UnhandledRaise(operation) => write!(
                f,
                "{operation} requires a surrounding 'try' block or enclosing function to declare 'raises'"
            ),
            Self::ContextManager(message) => f.write_str(message),
            Self::RaiseTypeMismatch { expected, found } => write!(
                f,
                "raising operation produces '{found}', but this context propagates '{expected}'"
            ),
            Self::NonCopyable { ty, context } => write!(
                f,
                "cannot copy non-Copyable type '{ty}' ({context}); transfer it with '^' \
                 or make '{ty}' Copyable"
            ),
            Self::ImplicitCopy {
                ty,
                context,
                transferable,
                copyable,
            } => {
                write!(
                    f,
                    "value of type '{ty}' cannot be implicitly copied, it does not conform to 'ImplicitlyCopyable' ({context})"
                )?;
                if *transferable {
                    write!(f, "; consider transferring the value with '^'")?;
                }
                if *copyable {
                    write!(f, "; you can copy it explicitly with '.copy()'")?;
                }
                Ok(())
            }
            Self::AliasingViolation { var } => write!(
                f,
                "'{var}' is borrowed mutably and also used at the same call \
                 (a mutable borrow must be exclusive)"
            ),
            Self::ExplicitDestroy {
                var,
                message,
                problem,
            } => write!(
                f,
                "explicit-destroy obligation for '{var}' {problem}: {message}"
            ),
            Self::NotCallable { name, ty } => {
                write!(f, "'{name}' has type {ty} and is not callable")
            }
            Self::ArityMismatch {
                name,
                expected,
                got,
            } => {
                write!(f, "'{name}' expects {expected} argument(s), got {got}")
            }
            Self::Redeclaration(name) => {
                write!(f, "'{name}' is already declared in this scope")
            }
            Self::ReservedName(name) => {
                write!(f, "'{name}' is a reserved word and cannot name a function")
            }
            Self::ImmutableBinding(name) => {
                write!(f, "expression must be mutable in assignment ('{name}')")
            }
            Self::ImmutableInPlaceDestination(name) => {
                write!(
                    f,
                    "expression must be mutable for in-place operator destination ('{name}')"
                )
            }
            Self::AssignToUndeclared(name) => {
                write!(
                    f,
                    "cannot assign to undeclared variable '{name}'; declare it with `var {name} = …`"
                )
            }
            Self::ReturnsReferenceToLocal => {
                write!(
                    f,
                    "returned reference escapes storage outside its declared origin"
                )
            }
            Self::PostInstantiation {
                receiver,
                method,
                error,
            } => {
                write!(f, "in '{method}' instantiated for '{receiver}': {error}")
            }
            Self::TransferEffectDivergence { rounds, callable } => {
                write!(
                    f,
                    "transfer-effect inference did not stabilize after {rounds} rounds; '{callable}' kept growing its effects"
                )
            }
            Self::StoredReferenceEscapesOrigin => {
                write!(
                    f,
                    "stored reference escapes storage outside its declared origin"
                )
            }
            Self::PointerEscapesOrigin => {
                write!(
                    f,
                    "returned pointer escapes storage outside its declared origin"
                )
            }
            Self::ClosureEscape => {
                write!(
                    f,
                    "closures cannot escape their defining scope (downward funargs only)"
                )
            }
            Self::ReturnOutsideFunction => write!(f, "'return' outside of a function"),
            Self::BreakOutsideLoop => write!(f, "'break' outside of a loop"),
            Self::ContinueOutsideLoop => write!(f, "'continue' outside of a loop"),
            Self::TypeMismatch {
                expected,
                found,
                context,
            } => {
                write!(
                    f,
                    "type mismatch for {context}: expected {expected}, found {found}"
                )
            }
            Self::BadOperator { op, operands } => {
                write!(f, "operator '{op}' is not defined for {operands}")
            }
            Self::MissingInPlaceOperator { op, ty } => {
                write!(
                    f,
                    "augmented assignment '{op}' requires an in-place method on '{ty}'"
                )
            }
            Self::UnknownType(name) => write!(f, "unknown type '{name}'"),
            Self::NoSuchField { object_type, field } => {
                write!(f, "type '{object_type}' has no field '{field}'")
            }
            Self::NoSuchAssociatedType {
                object_type,
                member,
            } => {
                write!(f, "type '{object_type}' has no associated type '{member}'")
            }
            Self::NoSuchMethod {
                object_type,
                method,
            } => {
                write!(f, "type '{object_type}' has no method '{method}'")
            }
            Self::NoConstructor(name) => {
                write!(
                    f,
                    "struct '{name}' has no constructor (add @fieldwise_init)"
                )
            }
            Self::UninitializedField {
                struct_name,
                method,
                field,
            } => {
                write!(
                    f,
                    "'{struct_name}.{method}' does not initialize field '{field}'"
                )
            }
            Self::ConflictingConstructor(name) => {
                write!(
                    f,
                    "struct '{name}' has both @fieldwise_init and a hand-written __init__"
                )
            }
            Self::UnknownTrait(name) => {
                write!(f, "unknown trait '{name}' in a type-parameter bound")
            }
            Self::ContextualMember { member, reason } => {
                write!(f, "cannot resolve leading '.{member}': {reason}")
            }
            Self::WrongTypeArgCount {
                name,
                expected,
                got,
            } => {
                write!(
                    f,
                    "type '{name}' expects {expected} type argument(s), got {got}"
                )
            }
            Self::UnknownSelfParam(name) => {
                write!(
                    f,
                    "'Self.{name}' is not a type parameter of the enclosing struct"
                )
            }
            Self::UnqualifiedStructParam(name) => write!(
                f,
                "unqualified access to struct parameter '{name}'; use 'Self.{name}' instead"
            ),
            Self::InstanceFieldWithoutInstance { field, ty } => write!(
                f,
                "cannot access instance field '{field}' without an instance of '{ty}'"
            ),
            Self::FieldNotDeinitable { field, ty } => {
                write!(f, "field '{field}' has non-'Deinitable' type '{ty}'")
            }
            Self::CannotInferTypeParam { name, param } => write!(
                f,
                "cannot infer type parameter '{param}' of '{name}' from the arguments"
            ),
            Self::TraitNotSatisfied {
                param,
                ty,
                trait_name,
                reason,
            } => {
                write!(
                    f,
                    "type '{ty}' for parameter '{param}' does not conform to trait '{trait_name}'"
                )?;
                if let Some(reason) = reason {
                    write!(f, ": {reason}")?;
                }
                Ok(())
            }
            Self::MissingTraitMethod {
                struct_name,
                trait_name,
                method,
            } => write!(
                f,
                "struct '{struct_name}' declares conformance to trait '{trait_name}' but is missing method '{method}'"
            ),
            Self::TraitMethodMismatch {
                struct_name,
                trait_name,
                method,
            } => write!(
                f,
                "struct '{struct_name}' method '{method}' does not match the signature required by trait '{trait_name}'"
            ),
            Self::MissingTraitComptimeMember {
                struct_name,
                trait_name,
                member,
            } => write!(
                f,
                "struct '{struct_name}' declares conformance to trait '{trait_name}' but is missing comptime member '{member}'"
            ),
            Self::TraitComptimeMemberMismatch {
                struct_name,
                trait_name,
                member,
            } => write!(
                f,
                "struct '{struct_name}' comptime member '{member}' does not match the requirement from trait '{trait_name}'"
            ),
            Self::BadValueParamType { name, ty } => {
                write!(f, "value parameter '{name}' must have type Int, not '{ty}'")
            }
            Self::NotComptime(what) => {
                write!(f, "not a compile-time Int constant: {what}")
            }
            Self::BadDtype(what) => write!(f, "not a valid SIMD element type: {what}"),
            Self::BadSimdWidth(w) => {
                write!(f, "SIMD width must be a positive power of two, got {w}")
            }
            Self::SimdArity { width, got } => write!(
                f,
                "SIMD construction expects {width} element(s) or 1 to splat, got {got}"
            ),
            Self::NotIndexable(ty) => {
                write!(f, "type '{ty}' cannot be indexed here")
            }
            Self::MissingReturn(name) => {
                write!(f, "'{name}' does not return a value on every path")
            }
            Self::MutationRequiresVariable(method) => write!(
                f,
                "'{method}' must be called on a plain list variable (mutating a temporary or field is not supported)"
            ),
            Self::ImmutableSelf => write!(
                f,
                "cannot assign to a field of 'self' in a method with a read-only receiver (use 'mut self')"
            ),
            Self::InvalidAssignTarget(what) => {
                write!(f, "invalid assignment target: {what}")
            }
            Self::Unsupported(what) => write!(f, "unsupported feature: {what}"),
            Self::InvariantViolation(detail) => {
                write!(f, "compiler invariant violated: {detail}")
            }
            Self::BadCall { func, reason } => {
                write!(f, "invalid call to '{func}': {reason}")
            }
            Self::UninitializedUse { var } => write!(
                f,
                "use of uninitialized value '{var}'\nnote: '{var}' declared here"
            ),
            Self::NoMatchingFunction { name } => {
                write!(f, "no matching function in call to '{name}'")
            }
            Self::Abandoned { var, message } => write!(
                f,
                "'{var}' abandoned without being explicitly destroyed: {message}"
            ),
            Self::LinearAbandoned { var, message } => write!(
                f,
                "'{var}' abandoned without being explicitly destroyed: {message}\nnote: consider adding trait conformance to Deinitable"
            ),
            Self::RefOriginMismatch {
                slot,
                ty,
                expected,
                actual,
            } => write!(
                f,
                "value passed to argument {slot} cannot be converted from '{ty}' to ref '{ty}'\nnote: operand origin '{actual}' doesn't match expected origin '{expected}'"
            ),
        }
    }
}

/// Errors from the ownership analysis (`analysis`), a compiler pass over the
/// MIR that runs after type-checking.
///
/// These model Mojo's move semantics — a value transferred with `^` is left
/// uninitialized, so using it again is an error. Each carries the source
/// `Span` (byte range) of the offending use, recovered from the MIR
/// `SpanTable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipError {
    /// Ownership analysis was requested for a program that did not pass semantic
    /// checking. The production compiler reports the earlier error directly;
    /// this variant protects the compatibility API from panicking.
    InvalidInput(String),
    /// A variable is used after it was transferred (`x^`) on every path to here.
    UseAfterMove {
        var: String,
        span: crate::token::SourceSpan,
    },
    /// A variable is used after being transferred on *some* (not all) paths — a
    /// move inside one branch of an `if`, then a use after the merge.
    ConditionallyMoved {
        var: String,
        span: crate::token::SourceSpan,
    },
    /// An owner place was accessed incompatibly while a local reference loan to
    /// overlapping storage remained live.
    LoanConflict {
        place: String,
        loan: String,
        span: crate::token::SourceSpan,
    },
    /// A reference into container-owned storage was used after an operation
    /// that may have replaced or reallocated that storage generation.
    InvalidatedInteriorReference {
        reference: String,
        origin: String,
        span: crate::token::SourceSpan,
        invalidated_at: Box<crate::token::SourceSpan>,
    },
}

impl fmt::Display for OwnershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(error) => {
                write!(f, "ownership analysis requires a checked program: {error}")
            }
            // Upstream reports a definite and a path-dependent transfer alike.
            Self::UseAfterMove { var, .. } | Self::ConditionallyMoved { var, .. } => write!(
                f,
                "use of uninitialized value '{var}'\nnote: '{var}' declared here"
            ),
            Self::LoanConflict { place, loan, .. } => write!(
                f,
                "access to '{place}' conflicts with live reference '{loan}'"
            ),
            Self::InvalidatedInteriorReference {
                reference,
                origin,
                invalidated_at,
                ..
            } => {
                let (start, end) = invalidated_at.span;
                write!(
                    f,
                    "use of invalidated interior reference '{reference}' to '{origin}'\n\
                     note: origin was invalidated here (bytes {start}..{end})"
                )
            }
        }
    }
}

impl OwnershipError {
    /// The source span (byte range) of the offending use.
    pub const fn span(&self) -> (usize, usize) {
        match self {
            Self::InvalidInput(_) => crate::token::DUMMY_SPAN,
            Self::UseAfterMove { span, .. }
            | Self::ConditionallyMoved { span, .. }
            | Self::LoanConflict { span, .. }
            | Self::InvalidatedInteriorReference { span, .. } => span.span,
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::InvalidInput(_) => None,
            Self::UseAfterMove { span, .. }
            | Self::ConditionallyMoved { span, .. }
            | Self::LoanConflict { span, .. }
            | Self::InvalidatedInteriorReference { span, .. } => span.source.as_deref(),
        }
    }

    /// The mutation/replacement site that invalidated an interior generation,
    /// when this is an invalidated-reference diagnostic.
    pub fn invalidation_span(&self) -> Option<&crate::token::SourceSpan> {
        match self {
            Self::InvalidatedInteriorReference { invalidated_at, .. } => {
                Some(invalidated_at.as_ref())
            }
            _ => None,
        }
    }
}

impl std::error::Error for LexError {}
impl std::error::Error for ParseError {}
impl std::error::Error for TypeError {}
impl std::error::Error for OwnershipError {}
