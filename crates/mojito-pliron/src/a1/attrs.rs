//! Typed attributes of `mojito_core`: every checked fact an operation needs
//! is a field of one of these, never a rendered string of MIR.

use pliron::combine::Parser;
use pliron::context::Context;
use pliron::derive::{format, pliron_attr};
use pliron::irfmt::parsers::quoted_string_parser;
use pliron::parsable::{Parsable, ParseResult, StateStream};
use pliron::printable::{self, Printable};
use pliron::r#type::TypeHandle;

use mojito_ast::ast::{ArgConvention, InfixOp, PrefixOp};
use mojito_checked::checked::{CheckedCallArgument, CheckedCallArgumentSource, CheckedConst};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::{SourceSpan, SyntaxId};
use mojito_mir::mir::{
    Const, MirCaptureAccess, MirCaptureMode, MirInteriorOrigin, MirIntrinsicSubscript, UseMode,
};
use mojito_types::types::SliceKind;

use super::params::{NodeKey, PayloadBinder};
use super::types::{CoreArg, CoreDtype, CorePlacePath, CoreSeg};
use super::{A1Error, A1ErrorKind};

/// A string in core text: double-quoted, with only `\\` and `"` escaped,
/// so every other character, newlines and non-ASCII included, is itself.
#[derive(Hash, PartialEq, Eq, Debug, Clone, PartialOrd, Ord, Default)]
pub struct Text(pub String);

impl Text {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for Text {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&String> for Text {
    fn from(value: &String) -> Self {
        Self(value.clone())
    }
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Printable for Text {
    fn fmt(
        &self,
        _ctx: &Context,
        _state: &printable::State,
        f: &mut core::fmt::Formatter<'_>,
    ) -> core::fmt::Result {
        write!(
            f,
            "\"{}\"",
            self.0.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

impl Parsable for Text {
    type Arg = ();
    type Parsed = Self;

    fn parse<'a>(
        state_stream: &mut StateStream<'a>,
        _arg: Self::Arg,
    ) -> ParseResult<'a, Self::Parsed> {
        quoted_string_parser()
            .map(Text)
            .parse_stream(state_stream)
            .into()
    }
}

/// A signed machine integer in core text. Pliron's own integer parser
/// reads digits alone, so a negative value needs its sign read here.
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord, Default)]
pub struct Signed(pub i64);

impl Printable for Signed {
    fn fmt(
        &self,
        _ctx: &Context,
        _state: &printable::State,
        f: &mut core::fmt::Formatter<'_>,
    ) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Parsable for Signed {
    type Arg = ();
    type Parsed = Self;

    fn parse<'a>(
        state_stream: &mut StateStream<'a>,
        _arg: Self::Arg,
    ) -> ParseResult<'a, Self::Parsed> {
        use pliron::combine::parser::char::{char, digit};
        use pliron::combine::{many1, optional};
        (optional(char('-')), many1::<String, _, _>(digit()))
            .and_then(|(sign, digits)| {
                let text = match sign {
                    Some(_) => format!("-{digits}"),
                    None => digits,
                };
                text.parse::<i64>().map(Signed)
            })
            .parse_stream(state_stream)
            .into()
    }
}

/// The stable identity of an operation: where its source instruction sat in
/// the imported function, and which part of that instruction it carries.
#[pliron_attr(
    name = "mojito_core.identity",
    format = "$function ` ` $region ` ` $block ` ` $ordinal ` ` $role",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone, PartialOrd, Ord)]
pub struct IdentityAttr {
    pub function: Text,
    /// The nested region path: empty for the function body.
    pub region: Text,
    pub block: u64,
    /// The instruction's position in its block; the block's instruction
    /// count for the terminator.
    pub ordinal: u64,
    pub role: CoreRole,
}

impl IdentityAttr {
    /// The identity as one line of text, for derivation records and maps.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{:?}",
            self.function, self.region, self.block, self.ordinal, self.role
        )
    }
}

/// Which part of its source instruction an operation carries.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord)]
pub enum CoreRole {
    /// The module container.
    Module,
    /// A function, as distinct from its first instruction.
    Function,
    /// The instruction or terminator itself.
    Primary,
    /// The `n`th place the instruction names.
    Place(u64),
    /// A load of a register slot feeding operand `n`.
    RegisterLoad(u64),
    /// The store of the instruction's result into its register slot.
    RegisterStore,
    /// The store of an `iter_next`'s has-element flag into its register slot.
    YieldStore,
    /// A variable or register slot declared at function entry.
    Slot(u64),
    /// The branch from a synthesized region entry to the region's block 0.
    Entry,
    /// The `n`th cleanup drop on a try's error edge.
    UnwindDrop(u64),
    /// The `n`th cleanup drop on a try's normal edge.
    DoneDrop(u64),
    /// The `n`th drop an exit crossing out of a try region runs first: the
    /// escape's own cleanup, then each enclosing try's, innermost first.
    ExitDrop(u64),
    /// The binding of a caught error to its handler's slot.
    Caught,
    /// The branch leaving a try's error-edge cleanup.
    UnwindExit,
    /// The branch leaving a try's normal-edge cleanup.
    DoneExit,
    /// The pending outcome of a normal, or an error, entry to `finally`.
    PendingNormal,
    PendingError,
    /// The branch from a pending outcome into `finally`.
    PendingNormalExit,
    PendingErrorExit,
    /// The pending outcome of a return or escape entering `finally`, and
    /// the branch into it.
    PendingExit,
    PendingExitExit,
    /// The terminator that ends an exit's way out after its last `finally`.
    ExitTerminal,
    /// The branch from the single `finally` entry to its block 0.
    FinallyEntry,
    /// The raise that hands an error out of the function.
    Propagate,
    /// An operation a test or pass inserted; `n` tells siblings apart.
    Inserted(u64),
}

/// Source provenance, kept apart from Pliron's assembly positions.
#[pliron_attr(
    name = "mojito_core.provenance",
    format = "`span` opt($span, delimiters(`(`, `)`)) ` origin` opt($origin, delimiters(`(`, `)`)) ` from [` vec($derived_from, CharSpace(`,`)) `] ` $reason",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ProvenanceAttr {
    /// The source record of the register this operation defines, exactly as
    /// the input carried it; `None` when the input had none.
    pub span: Option<CoreSpan>,
    pub origin: Option<u32>,
    /// The identities this operation was synthesized from, when it has no
    /// source record of its own.
    pub derived_from: Vec<Text>,
    pub reason: Text,
}

#[format(
    "`source` opt($source, delimiters(`(`, `)`)) ` ` $start ` ` $end ` syntax` opt($syntax, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreSpan {
    pub source: Option<Text>,
    pub start: u64,
    pub end: u64,
    pub syntax: Option<u64>,
}

impl CoreSpan {
    pub fn from_span(span: &SourceSpan) -> Self {
        Self {
            source: span.source.clone().map(Text::from),
            start: span.span.0 as u64,
            end: span.span.1 as u64,
            syntax: span.syntax.map(|syntax| syntax.0),
        }
    }

    pub fn span(&self) -> Result<SourceSpan, A1Error> {
        let offset = |value: u64| {
            usize::try_from(value).map_err(|_| {
                A1Error::new(
                    A1ErrorKind::Export,
                    "a source offset beyond the address space",
                )
            })
        };
        Ok(SourceSpan {
            source: self.source.clone().map(|source| source.0),
            span: (offset(self.start)?, offset(self.end)?),
            syntax: self.syntax.map(SyntaxId),
        })
    }
}

/// The MIR register an operation's value result defines.
#[pliron_attr(name = "mojito_core.reg", format = "$0", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct RegAttr(pub u32);

/// A slot's storage identity and ownership flags.
#[pliron_attr(
    name = "mojito_core.slot_info",
    format = "$storage ` ` $id ` ` $name ` param` opt($param, delimiters(`(`, `)`)) ` ` $owned ` ` $deinit ` ` $by_ref",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SlotAttr {
    pub storage: CoreStorage,
    /// The `VarId` of a variable slot, or the `Reg` of a register slot.
    pub id: u32,
    pub name: Text,
    /// The parameter position this slot binds.
    pub param: Option<u32>,
    pub owned: bool,
    pub deinit: bool,
    pub by_ref: bool,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreStorage {
    Variable,
    Register,
}

#[pliron_attr(name = "mojito_core.constant", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum ConstAttr {
    Int(Signed),
    /// An exact integer literal in decimal, of any magnitude.
    IntLiteral(Text),
    /// The bits of a machine `Float64`, so every value round-trips.
    Float(u64),
    /// An exact floating literal in its own spelling: `-0.0`, `{n}.0`, or
    /// a reduced `{numerator}/{denominator}`.
    FloatLiteral(Text),
    Bool(bool),
    Str(Text),
    None,
    Dtype(CoreDtype),
    /// A function value: the symbol of the body it names.
    Function(Text),
}

impl ConstAttr {
    pub fn from_const(constant: &Const) -> Result<Self, A1Error> {
        match constant {
            Const::Int(value) => Ok(Self::Int(Signed(*value))),
            Const::IntLiteral(literal) => {
                Ok(Self::IntLiteral(literal.as_bigint().to_string().into()))
            }
            Const::Float(value) => Ok(Self::Float(value.to_bits())),
            Const::FloatLiteral(literal) => Ok(Self::FloatLiteral(literal.to_string().into())),
            Const::Bool(value) => Ok(Self::Bool(*value)),
            Const::Str(value) => Ok(Self::Str(value.into())),
            Const::None => Ok(Self::None),
            Const::Dtype(dtype) => Ok(Self::Dtype(CoreDtype::from_dtype(*dtype))),
            Const::Function(symbol) => Ok(Self::Function(symbol.into())),
        }
    }

    pub fn constant(&self) -> Result<Const, A1Error> {
        Ok(match self {
            Self::Int(value) => Const::Int(value.0),
            Self::IntLiteral(digits) => Const::IntLiteral(
                IntLiteral::parse_radix(digits.as_str(), 10).ok_or_else(|| {
                    A1Error::new(
                        A1ErrorKind::Export,
                        format!("`{digits}` is not a decimal integer literal"),
                    )
                })?,
            ),
            Self::Float(bits) => Const::Float(f64::from_bits(*bits)),
            Self::FloatLiteral(text) => Const::FloatLiteral(parse_float_literal(text)?),
            Self::Bool(value) => Const::Bool(*value),
            Self::Str(value) => Const::Str(value.0.clone()),
            Self::None => Const::None,
            Self::Dtype(dtype) => Const::Dtype(dtype.dtype()),
            Self::Function(symbol) => Const::Function(symbol.0.clone()),
        })
    }
}

/// The exact floating literal `text` spells, in the literal's own
/// `Display` form.
pub fn parse_float_literal(text: &Text) -> Result<FloatLiteral, A1Error> {
    FloatLiteral::parse_exact(text.as_str()).ok_or_else(|| {
        A1Error::new(
            A1ErrorKind::Export,
            format!("`{text}` is not an exact floating literal"),
        )
    })
}

/// The lane type and width a SIMD construction declares. Its register
/// holds that vector, or the scalar alias of a one-lane vector.
#[pliron_attr(
    name = "mojito_core.simd_shape",
    format = "$dtype ` x ` $width",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SimdMakeAttr {
    pub dtype: CoreDtype,
    pub width: u64,
}

#[pliron_attr(name = "mojito_core.infix", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum InfixAttr {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    MatMul,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
}

impl InfixAttr {
    pub const fn from_op(op: InfixOp) -> Self {
        match op {
            InfixOp::Add => Self::Add,
            InfixOp::Sub => Self::Sub,
            InfixOp::Mul => Self::Mul,
            InfixOp::Div => Self::Div,
            InfixOp::FloorDiv => Self::FloorDiv,
            InfixOp::Mod => Self::Mod,
            InfixOp::MatMul => Self::MatMul,
            InfixOp::Shl => Self::Shl,
            InfixOp::Shr => Self::Shr,
            InfixOp::BitAnd => Self::BitAnd,
            InfixOp::BitOr => Self::BitOr,
            InfixOp::BitXor => Self::BitXor,
            InfixOp::Pow => Self::Pow,
            InfixOp::Eq => Self::Eq,
            InfixOp::Ne => Self::Ne,
            InfixOp::Lt => Self::Lt,
            InfixOp::Gt => Self::Gt,
            InfixOp::Le => Self::Le,
            InfixOp::Ge => Self::Ge,
            InfixOp::And => Self::And,
            InfixOp::Or => Self::Or,
            InfixOp::In => Self::In,
            InfixOp::NotIn => Self::NotIn,
            InfixOp::Is => Self::Is,
            InfixOp::IsNot => Self::IsNot,
        }
    }

    pub const fn op(self) -> InfixOp {
        match self {
            Self::Add => InfixOp::Add,
            Self::Sub => InfixOp::Sub,
            Self::Mul => InfixOp::Mul,
            Self::Div => InfixOp::Div,
            Self::FloorDiv => InfixOp::FloorDiv,
            Self::Mod => InfixOp::Mod,
            Self::MatMul => InfixOp::MatMul,
            Self::Shl => InfixOp::Shl,
            Self::Shr => InfixOp::Shr,
            Self::BitAnd => InfixOp::BitAnd,
            Self::BitOr => InfixOp::BitOr,
            Self::BitXor => InfixOp::BitXor,
            Self::Pow => InfixOp::Pow,
            Self::Eq => InfixOp::Eq,
            Self::Ne => InfixOp::Ne,
            Self::Lt => InfixOp::Lt,
            Self::Gt => InfixOp::Gt,
            Self::Le => InfixOp::Le,
            Self::Ge => InfixOp::Ge,
            Self::And => InfixOp::And,
            Self::Or => InfixOp::Or,
            Self::In => InfixOp::In,
            Self::NotIn => InfixOp::NotIn,
            Self::Is => InfixOp::Is,
            Self::IsNot => InfixOp::IsNot,
        }
    }
}

#[pliron_attr(name = "mojito_core.prefix", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum PrefixAttr {
    Neg,
    Not,
    Invert,
}

impl PrefixAttr {
    pub const fn from_op(op: PrefixOp) -> Self {
        match op {
            PrefixOp::Neg => Self::Neg,
            PrefixOp::Not => Self::Not,
            PrefixOp::Invert => Self::Invert,
        }
    }

    pub const fn op(self) -> PrefixOp {
        match self {
            Self::Neg => PrefixOp::Neg,
            Self::Not => PrefixOp::Not,
            Self::Invert => PrefixOp::Invert,
        }
    }
}

/// Which lane-wise conversion a `simd_convert` is.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreSimdConversion {
    Cast,
    Bits,
}

/// A `simd_convert`: its conversion, and the lane type and width it
/// declares. Its register holds that vector, or the scalar alias of a
/// one-lane vector.
#[pliron_attr(
    name = "mojito_core.simd_conversion",
    format = "$conversion ` ` $dtype ` x ` $width",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SimdConvertAttr {
    pub conversion: CoreSimdConversion,
    pub dtype: CoreDtype,
    pub width: u64,
}

/// A checker-selected symbol, when one was required.
#[pliron_attr(
    name = "mojito_core.resolved",
    format = "`resolved` opt($0, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ResolvedAttr(pub Option<Text>);

#[pliron_attr(name = "mojito_core.use_mode", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum UseModeAttr {
    Copy,
    Move,
    BorrowShared,
    BorrowMut,
}

impl UseModeAttr {
    pub const fn from_mode(mode: UseMode) -> Self {
        match mode {
            UseMode::Copy => Self::Copy,
            UseMode::Move => Self::Move,
            UseMode::BorrowShared => Self::BorrowShared,
            UseMode::BorrowMut => Self::BorrowMut,
        }
    }

    pub const fn mode(self) -> UseMode {
        match self {
            Self::Copy => UseMode::Copy,
            Self::Move => UseMode::Move,
            Self::BorrowShared => UseMode::BorrowShared,
            Self::BorrowMut => UseMode::BorrowMut,
        }
    }
}

/// What a store initializes: a variable definition with its checked
/// binding type, a write through a place, or register transport.
#[pliron_attr(name = "mojito_core.store_kind", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum StoreAttr {
    #[format("`<binding` opt($binding, delimiters(`(`, `)`)) `>`")]
    DefVar {
        binding: Option<TypeHandle>,
    },
    Place,
    Register,
    /// The caught error, bound to the handler's slot on the error edge.
    Caught,
}

/// The typed projection path of a place below its root slot.
#[pliron_attr(
    name = "mojito_core.projection",
    format = "`root` opt($root_ty, delimiters(`(`, `)`)) ` [` vec($steps, CharSpace(`,`)) `] type` opt($ty, delimiters(`(`, `)`)) ` through` opt($through, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ProjectionAttr {
    pub root_ty: Option<TypeHandle>,
    pub steps: Vec<CoreStep>,
    pub ty: Option<TypeHandle>,
    pub through: Option<u32>,
}

#[format("$kind ` : ` $ty")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreStep {
    pub kind: CoreStepKind,
    pub ty: TypeHandle,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreStepKind {
    Field(Text),
    /// A dynamic index: the next index operand of the projection.
    Index,
    ConstIndex(u64),
    Variant(u64),
    UninitPayload,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreCallKind {
    Direct,
    Method,
    /// A call through a function value, which is the first operand.
    Indirect,
}

/// One static read or write of captured owner storage a call performs
/// transitively.
#[format("$place ` ` $write")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreCaptureAccess {
    pub place: CorePlacePath,
    pub write: bool,
}

impl CoreCaptureAccess {
    pub fn from_access(access: &MirCaptureAccess) -> Self {
        Self {
            place: CorePlacePath {
                root: access.root,
                path: access.path.iter().map(CoreSeg::from_seg).collect(),
            },
            write: access.access == mojito_types::origin::CaptureAccess::Write,
        }
    }

    pub fn access(&self) -> MirCaptureAccess {
        MirCaptureAccess {
            root: self.place.root,
            path: self.place.path.iter().map(CoreSeg::seg).collect(),
            access: if self.write {
                mojito_types::origin::CaptureAccess::Write
            } else {
                mojito_types::origin::CaptureAccess::Read
            },
        }
    }
}

/// How a closure takes one captured place.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreCaptureMode {
    Reference,
    Copy,
    Move,
}

impl CoreCaptureMode {
    pub const fn from_mode(mode: MirCaptureMode) -> Self {
        match mode {
            MirCaptureMode::Reference => Self::Reference,
            MirCaptureMode::Copy => Self::Copy,
            MirCaptureMode::Move => Self::Move,
        }
    }

    pub const fn mode(self) -> MirCaptureMode {
        match self {
            Self::Reference => MirCaptureMode::Reference,
            Self::Copy => MirCaptureMode::Copy,
            Self::Move => MirCaptureMode::Move,
        }
    }
}

/// The lifted body a closure runs and how it takes each captured place,
/// in operand order.
#[pliron_attr(
    name = "mojito_core.closure",
    format = "$function ` [` vec($modes, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ClosureAttr {
    pub function: Text,
    pub modes: Vec<CoreCaptureMode>,
}

/// One compile-time argument of a call.
#[format(
    "`name` opt($name, delimiters(`(`, `)`)) ` ` $value ` binder` opt($binder, delimiters(`(`, `)`)) ` expr` opt($expr, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreParamArg {
    pub name: Option<Text>,
    /// Whether a register reifies the argument, as an operand.
    pub value: bool,
    /// The enclosing declaration's type binder the argument forwards.
    pub binder: Option<PayloadBinder>,
    /// The expression over the enclosing declaration's value binders.
    pub expr: Option<NodeKey>,
}

/// The checked facts of a call.
///
/// Operands are segmented as receiver, positional arguments, keyword
/// arguments, reified compile-time arguments, retained argument places,
/// retained keyword places, the receiver place, and the effect token.
#[pliron_attr(
    name = "mojito_core.call_facts",
    format = "$kind ` ` $target ` resolved` opt($resolved, delimiters(`(`, `)`)) ` raises` opt($raises, delimiters(`(`, `)`)) ` args ` $args ` kwargs [` vec($kwargs, CharSpace(`,`)) `] places [` vec($arg_places, CharSpace(`,`)) `] kwplaces [` vec($kwarg_places, CharSpace(`,`)) `] ` $recv_place ` ` $recv_writes ` params [` vec($params, CharSpace(`,`)) `] captures [` vec($captures, CharSpace(`,`)) `] contract` opt($contract, delimiters(`(`, `)`)) ` instantiated [` vec($instantiated, CharSpace(`,`)) `] reference` opt($reference_result, delimiters(`(`, `)`)) ` adapter` opt($adapter, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CallAttr {
    pub kind: CoreCallKind,
    /// The callee symbol of a direct call, or the method's source name.
    pub target: Text,
    pub resolved: Option<Text>,
    pub raises: Option<TypeHandle>,
    /// The count of positional arguments.
    pub args: u64,
    pub kwargs: Vec<Text>,
    /// Whether each positional argument retains a caller place; empty when
    /// the call records no place table.
    pub arg_places: Vec<bool>,
    pub kwarg_places: Vec<bool>,
    /// Whether the receiver, or an indirect call's callee, retains a place.
    pub recv_place: bool,
    pub recv_writes: bool,
    pub params: Vec<CoreParamArg>,
    /// The captured owner storage the call reads or writes transitively.
    pub captures: Vec<CoreCaptureAccess>,
    /// The callable contract the checker instantiated for a generic
    /// indirect call.
    pub contract: Option<TypeHandle>,
    /// The retained generic arguments of that instantiation.
    pub instantiated: Vec<CoreArg>,
    /// The instantiated reference type of a method's reference result,
    /// kept apart from the result register's type.
    pub reference_result: Option<TypeHandle>,
    /// The adapter the call site applies to an abstract result.
    pub adapter: Option<CoreResultAdapter>,
}

impl CallAttr {
    /// The operand count of each segment but the trailing effect token:
    /// the receiver or callee, the positional and keyword arguments, the
    /// reified compile-time arguments, then the retained places.
    pub fn segments(&self) -> [usize; 7] {
        let retained = |places: &[bool]| places.iter().filter(|place| **place).count();
        [
            usize::from(self.kind != CoreCallKind::Direct),
            usize::try_from(self.args).unwrap_or(usize::MAX),
            self.kwargs.len(),
            self.params.iter().filter(|param| param.value).count(),
            retained(&self.arg_places),
            retained(&self.kwarg_places),
            usize::from(self.recv_place),
        ]
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreArgSource {
    Positional(u64),
    Keyword(u64),
    Default,
}

/// One argument of a selected method-like call, as the checker bound it.
#[format(
    "$source ` : ` $parameter ` ` $requires_place ` convention` opt($convention, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreCallArgument {
    pub source: CoreArgSource,
    pub parameter: TypeHandle,
    pub requires_place: bool,
    pub convention: Option<CoreConvention>,
}

impl CoreCallArgument {
    pub fn argument(&self, ctx: &Context) -> Result<CheckedCallArgument, A1Error> {
        let index = |index: u64| {
            usize::try_from(index)
                .map_err(|_| A1Error::new(A1ErrorKind::Export, "an argument index beyond usize"))
        };
        Ok(CheckedCallArgument {
            source: match self.source {
                CoreArgSource::Positional(position) => {
                    CheckedCallArgumentSource::Positional(index(position)?)
                }
                CoreArgSource::Keyword(position) => {
                    CheckedCallArgumentSource::Keyword(index(position)?)
                }
                CoreArgSource::Default => CheckedCallArgumentSource::Default,
            },
            parameter_ty: super::types::export_type(ctx, self.parameter)?,
            requires_place: self.requires_place,
            convention: self.convention.map(CoreConvention::convention),
        })
    }
}

/// The nominal call a subscript dispatches to. Its reified compile-time
/// arguments are operands.
#[format(
    "$target ` raises` opt($raises, delimiters(`(`, `)`)) ` -> ` $result ` ` $receiver_requires_place ` receiver` opt($receiver_convention, delimiters(`(`, `)`)) ` arguments [` vec($arguments, CharSpace(`,`)) `] reference` opt($reference_result, delimiters(`(`, `)`)) ` params [` vec($params, CharSpace(`,`)) `] captures [` vec($captures, CharSpace(`,`)) `]`"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreSubscriptCall {
    pub target: Text,
    pub raises: Option<TypeHandle>,
    pub result: TypeHandle,
    pub receiver_requires_place: bool,
    pub receiver_convention: Option<CoreConvention>,
    pub arguments: Vec<CoreCallArgument>,
    /// The instantiated reference type of a reference result.
    pub reference_result: Option<TypeHandle>,
    pub params: Vec<CoreParamArg>,
    pub captures: Vec<CoreCaptureAccess>,
}

impl CoreSubscriptCall {
    /// The count of operands reifying compile-time arguments.
    pub fn reified(&self) -> usize {
        self.params.iter().filter(|param| param.value).count()
    }
}

/// The compiler storage family a subscript without a nominal call reads.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreIntrinsic {
    TupleStorage,
    VariadicStorage,
    Simd,
    Pointer,
    ComptimeList,
}

impl CoreIntrinsic {
    pub const fn from_intrinsic(intrinsic: MirIntrinsicSubscript) -> Self {
        match intrinsic {
            MirIntrinsicSubscript::TupleStorage => Self::TupleStorage,
            MirIntrinsicSubscript::VariadicStorage => Self::VariadicStorage,
            MirIntrinsicSubscript::Simd => Self::Simd,
            MirIntrinsicSubscript::Pointer => Self::Pointer,
            MirIntrinsicSubscript::ComptimeList => Self::ComptimeList,
        }
    }

    pub const fn intrinsic(self) -> MirIntrinsicSubscript {
        match self {
            Self::TupleStorage => MirIntrinsicSubscript::TupleStorage,
            Self::VariadicStorage => MirIntrinsicSubscript::VariadicStorage,
            Self::Simd => MirIntrinsicSubscript::Simd,
            Self::Pointer => MirIntrinsicSubscript::Pointer,
            Self::ComptimeList => MirIntrinsicSubscript::ComptimeList,
        }
    }
}

/// The field a value's field read names.
#[pliron_attr(name = "mojito_core.field", format = "$name", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct FieldAttr {
    pub name: Text,
}

/// The type whose target-layout byte size a `size_of` yields.
#[pliron_attr(name = "mojito_core.size_of", format = "$ty", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SizeOfAttr {
    pub ty: TypeHandle,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreSliceKind {
    Slice,
    ContiguousSlice,
    StridedSlice,
}

impl CoreSliceKind {
    pub const fn from_kind(kind: SliceKind) -> Self {
        match kind {
            SliceKind::Slice => Self::Slice,
            SliceKind::ContiguousSlice => Self::ContiguousSlice,
            SliceKind::StridedSlice => Self::StridedSlice,
        }
    }

    pub const fn kind(self) -> SliceKind {
        match self {
            Self::Slice => SliceKind::Slice,
            Self::ContiguousSlice => SliceKind::ContiguousSlice,
            Self::StridedSlice => SliceKind::StridedSlice,
        }
    }
}

/// A slice's kind and which of its bounds it spells, each a value operand
/// when present.
#[format("$kind ` ` $lower ` ` $upper ` ` $step")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreSliceBounds {
    pub kind: CoreSliceKind,
    pub lower: bool,
    pub upper: bool,
    pub step: bool,
}

impl CoreSliceBounds {
    /// The value operands the bounds take.
    pub fn values(&self) -> usize {
        usize::from(self.lower) + usize::from(self.upper) + usize::from(self.step)
    }
}

/// One argument of a multi-argument subscript: an index value, or a
/// slice with the bounds it spells.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreSubscriptArg {
    Index,
    Slice(CoreSliceBounds),
}

impl CoreSubscriptArg {
    /// The value operands the argument takes.
    pub fn values(&self) -> usize {
        match self {
            Self::Index => 1,
            Self::Slice(bounds) => bounds.values(),
        }
    }
}

/// A keyword argument of a multi-argument subscript.
#[format("$name ` ` $arg")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreKeywordArg {
    pub name: Text,
    pub arg: CoreSubscriptArg,
}

/// The checked dispatch of a `multi_index`: its arguments in operand
/// order, then which of the object and the arguments retain a place.
#[pliron_attr(
    name = "mojito_core.multi_subscript",
    format = "`[` vec($args, CharSpace(`,`)) `] kwargs [` vec($kwargs, CharSpace(`,`)) `] ` $object_place ` [` vec($arg_places, CharSpace(`,`)) `] [` vec($kwarg_places, CharSpace(`,`)) `] call` opt($call, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct MultiIndexAttr {
    pub args: Vec<CoreSubscriptArg>,
    pub kwargs: Vec<CoreKeywordArg>,
    pub object_place: bool,
    pub arg_places: Vec<bool>,
    pub kwarg_places: Vec<bool>,
    pub call: Option<CoreSubscriptCall>,
}

impl MultiIndexAttr {
    /// The value operands before the reified compile-time arguments: the
    /// object and every argument value.
    pub fn values(&self) -> usize {
        1 + self
            .args
            .iter()
            .map(CoreSubscriptArg::values)
            .sum::<usize>()
            + self
                .kwargs
                .iter()
                .map(|keyword| keyword.arg.values())
                .sum::<usize>()
    }

    pub fn places(&self) -> usize {
        let retained = |places: &[bool]| places.iter().filter(|place| **place).count();
        usize::from(self.object_place) + retained(&self.arg_places) + retained(&self.kwarg_places)
    }
}

/// The checked dispatch of a `slice`: which bounds it spells, which of
/// the object and the arguments retain a place, and its target.
#[pliron_attr(
    name = "mojito_core.slice_subscript",
    format = "$bounds ` ` $object_place ` [` vec($arg_places, CharSpace(`,`)) `] call` opt($call, delimiters(`(`, `)`)) ` intrinsic` opt($intrinsic, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SliceAttr {
    pub bounds: CoreSliceBounds,
    pub object_place: bool,
    pub arg_places: Vec<bool>,
    pub call: Option<CoreSubscriptCall>,
    pub intrinsic: Option<CoreIntrinsic>,
}

impl SliceAttr {
    pub fn values(&self) -> usize {
        1 + self.bounds.values()
    }

    pub fn places(&self) -> usize {
        usize::from(self.object_place) + self.arg_places.iter().filter(|place| **place).count()
    }
}

/// The lane selection of a SIMD shuffle, over one vector or two.
#[pliron_attr(
    name = "mojito_core.shuffle",
    format = "$other ` [` vec($mask, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ShuffleAttr {
    pub other: bool,
    pub mask: Vec<u64>,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreUninitAccess {
    Make,
    Take,
    Destroy,
}

/// What an `uninit_storage` does: builds storage, optionally from a
/// value, or takes or destroys the payload of the element type.
#[pliron_attr(
    name = "mojito_core.uninit",
    format = "$access ` ` $init ` element` opt($element, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct UninitAttr {
    pub access: CoreUninitAccess,
    pub init: bool,
    pub element: Option<TypeHandle>,
}

/// What a variant operation does, with the alternative it names.
///
/// A construction, a tag test, a payload read or take, a write of a
/// payload or of a factory's result, a replacement, or a consuming
/// handler call.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreVariantAccess {
    Make(u64),
    Test(u64),
    Get(u64),
    Take(u64, bool),
    Set(u64),
    Replace(u64, u64, bool),
    SetInitWith(u64),
    DeinitWith(u64),
}

#[pliron_attr(
    name = "mojito_core.variant_access",
    format = "$access",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct VariantAttr {
    pub access: CoreVariantAccess,
}

/// The checked dispatch of an `index`.
///
/// Operands are the base, the index, the reified compile-time arguments,
/// the retained base and index places, and the effect token.
#[pliron_attr(
    name = "mojito_core.subscript",
    format = "$base_place ` ` $index_place ` call` opt($call, delimiters(`(`, `)`)) ` intrinsic` opt($intrinsic, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct IndexAttr {
    pub base_place: bool,
    pub index_place: bool,
    pub call: Option<CoreSubscriptCall>,
    pub intrinsic: Option<CoreIntrinsic>,
}

/// The checked dispatch of a `multi_set`.
///
/// Operands are the receiver, the index arguments, the stored value, the
/// reified compile-time arguments, the retained receiver, argument, and
/// value places, and the effect token.
#[pliron_attr(
    name = "mojito_core.subscript_store",
    format = "$receiver_place ` places [` vec($arg_places, CharSpace(`,`)) `] ` $value_place ` ` $value_keyword ` call(` $call `)`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct MultiSetAttr {
    pub receiver_place: bool,
    /// Whether each index argument retains a caller place.
    pub arg_places: Vec<bool>,
    pub value_place: bool,
    pub value_keyword: bool,
    pub call: CoreSubscriptCall,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CorePointerAccess {
    Take,
    Destroy,
}

#[pliron_attr(
    name = "mojito_core.pointer_access",
    format = "$access ` (` $element `)`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct PointerStorageAttr {
    pub access: CorePointerAccess,
    pub element: TypeHandle,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreIterationMode {
    Borrowed,
    Owned,
}

#[pliron_attr(
    name = "mojito_core.iteration",
    format = "$mode `, prepare [` vec($prepare, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct IterInitAttr {
    pub mode: CoreIterationMode,
    pub prepare: Vec<Text>,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreResultAdapter {
    CopyIteratorReference,
}

/// The selected `__next__` of an `iter_next`, and the register and source
/// record of its second result: whether an element was yielded.
#[pliron_attr(
    name = "mojito_core.iterator_call",
    format = "$target ` -> ` $result ` reference` opt($reference_result, delimiters(`(`, `)`)) ` raises` opt($raises, delimiters(`(`, `)`)) ` adapter` opt($adapter, delimiters(`(`, `)`)) ` exhaustion(` $exhaustion `) yields ` $yielded ` ` $yielded_record",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct IterNextAttr {
    pub target: Text,
    pub result: TypeHandle,
    pub reference_result: Option<TypeHandle>,
    pub raises: Option<TypeHandle>,
    pub adapter: Option<CoreResultAdapter>,
    pub exhaustion: TypeHandle,
    pub yielded: u32,
    pub yielded_record: CoreOrphanSpan,
}

/// The count of cleanup slots a `return` carries out of its loops.
#[pliron_attr(name = "mojito_core.cleanup", format = "$0", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct CleanupAttr(pub u64);

/// A lifecycle event: what is destroyed or consumed, and its stable key.
#[pliron_attr(
    name = "mojito_core.lifecycle",
    format = "$kind ` ` $owner ` [` vec($path, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct LifecycleAttr {
    pub kind: CoreLifecycle,
    /// The `VarId` owning the destroyed storage; a register's number for a
    /// register drop.
    pub owner: u32,
    pub path: Vec<CoreSeg>,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreLifecycle {
    DropVar,
    DropPlace,
    DropReg,
    ConsumeVar,
    ConsumePlace,
}

#[format("`var ` $root ` [` vec($path, CharSpace(`,`)) `]`")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreInterior {
    pub root: u32,
    pub path: Vec<CoreSeg>,
}

impl CoreInterior {
    pub fn from_interior(interior: &MirInteriorOrigin) -> Self {
        Self {
            root: interior.root,
            path: interior.path.iter().map(CoreSeg::from_seg).collect(),
        }
    }

    pub fn interior(&self) -> MirInteriorOrigin {
        MirInteriorOrigin {
            root: self.root,
            path: self.path.iter().map(CoreSeg::seg).collect(),
        }
    }
}

#[format("$mutable ` ` $shared ` interior` opt($interior, delimiters(`(`, `)`))")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreLoan {
    pub mutable: bool,
    pub shared: bool,
    pub interior: Option<CoreInterior>,
}

/// One generation of loans: each loan's place is an operand, in order.
#[pliron_attr(
    name = "mojito_core.loan_set",
    format = "`[` vec($loans, CharSpace(`,`)) `] dest` opt($dest_interior, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct LoansAttr {
    pub loans: Vec<CoreLoan>,
    pub dest_interior: Option<CoreInterior>,
}

#[pliron_attr(
    name = "mojito_core.invalidation",
    format = "$base ` except` opt($except, delimiters(`(`, `)`)) ` ` $include_base",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct InvalidateAttr {
    pub base: CoreInterior,
    pub except: Option<u32>,
    pub include_base: bool,
}

/// The parts a structured `try` has, and the slot its handler binds.
#[pliron_attr(
    name = "mojito_core.try_parts",
    format = "$handler ` error` opt($error_var, delimiters(`(`, `)`)) ` ` $orelse ` ` $finalbody",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct TryAttr {
    pub handler: bool,
    pub error_var: Option<u32>,
    pub orelse: bool,
    pub finalbody: bool,
}

/// How a block of a structured region hands control out of it.
#[pliron_attr(name = "mojito_core.exit_kind", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum ExitAttr {
    FallOff,
}

/// The function-level block a `break` or `continue` inside a try region
/// leaves for.
#[pliron_attr(
    name = "mojito_core.escape_target",
    format = "$target",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct EscapeAttr {
    pub target: u64,
}

/// The MIR terminator a `raise` cuts off: MIR keeps `raise` an instruction,
/// so the block's own terminator follows it unreachably.
#[pliron_attr(name = "mojito_core.dead_term", format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum DeadTermAttr {
    Jump(u64),
    Return,
    FallOff,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum OutcomeKind {
    Normal,
    Error,
    /// A return or escape crossing the `finally`, by exit site.
    Exit(u64),
}

/// The exit sites a `resume` continues, one successor each after its
/// normal and error successors.
#[pliron_attr(
    name = "mojito_core.resume_sites",
    format = "`[` vec($sites, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ResumeAttr {
    pub sites: Vec<u64>,
}

/// The exit site a branch into a pending exit stands for: the return or
/// escape crossing a `finally`, and whether it carries a value.
#[pliron_attr(
    name = "mojito_core.exit_site",
    format = "$site ` ` $value",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct ExitSiteAttr {
    pub site: u64,
    pub value: bool,
}

/// The pending outcome a `finally` body runs under.
#[pliron_attr(name = "mojito_core.outcome_kind", format = "$kind", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct OutcomeAttr {
    pub kind: OutcomeKind,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum BlockCategory {
    /// The function entry: slots, then the branch to block 0.
    Entry,
    /// One segment of a MIR block; an invoke or a try ends a segment.
    Body,
    /// A try's error edge: cleanup, then the handler or the way out.
    Unwind,
    /// A try's normal edge: cleanup, then `else` or the way on.
    BodyDone,
    PendingNormal,
    PendingError,
    /// A return or escape entering `finally` with its outcome pending.
    PendingExit,
    /// Where a pending exit continues after `finally`.
    ExitContinue,
    FinallyEntry,
    Propagate,
}

/// Where one block of a normalized function belongs.
#[format("$category ` ` $region ` ` $block ` ` $segment")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct BlockRecord {
    pub category: BlockCategory,
    /// The region path of a body segment; the try's path otherwise.
    pub region: Text,
    pub block: u64,
    pub segment: u64,
}

/// The region layout of a normalized function, one record per block in
/// block order: membership and boundaries only, never an instruction.
#[pliron_attr(
    name = "mojito_core.layout",
    format = "`[` vec($0, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct LayoutAttr(pub Vec<BlockRecord>);

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum EventKind {
    Init,
    Move,
    Drop,
    Consume,
}

/// One lifecycle event of the imported schedule: where it sits in the
/// effect chain, what it owns, and the states its owner may be in before.
#[format("$key ` ` $kind ` ` $owner ` ` $block ` ` $position ` ` $before")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreEvent {
    pub key: Text,
    pub kind: EventKind,
    pub owner: u32,
    /// The block, by its position in the function.
    pub block: u64,
    /// The position among the block's effectful operations.
    pub position: u64,
    /// The owner's possible states before the event, as state bits.
    pub before: u8,
}

/// The cleanup contract of a function, derived once from verified input
/// and rechecked against the executable operations.
#[pliron_attr(
    name = "mojito_core.contract",
    format = "`[` vec($0, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ContractAttr(pub Vec<CoreEvent>);

/// A function's checked signature and frame shape.
#[pliron_attr(
    name = "mojito_core.signature",
    format = "$symbol ` registers ` $registers ` returns` opt($ret, delimiters(`(`, `)`)) ` ` $returns_reference ` ` $raises ` error` opt($error, delimiters(`(`, `)`)) ` params [` vec($params, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct SignatureAttr {
    /// The exact Mojito symbol, which no Pliron identifier can spell.
    pub symbol: Text,
    pub registers: u32,
    pub ret: Option<TypeHandle>,
    pub returns_reference: bool,
    pub raises: bool,
    pub error: Option<TypeHandle>,
    pub params: Vec<TypeHandle>,
}

/// A register no operation defines, kept so the frame's tables survive.
#[format("$reg ` type` opt($ty, delimiters(`(`, `)`)) ` ` $provenance")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreOrphan {
    pub reg: u32,
    pub ty: Option<TypeHandle>,
    pub provenance: CoreOrphanSpan,
}

#[format("`span` opt($span, delimiters(`(`, `)`)) ` origin` opt($origin, delimiters(`(`, `)`))")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreOrphanSpan {
    pub span: Option<CoreSpan>,
    pub origin: Option<u32>,
}

#[pliron_attr(
    name = "mojito_core.orphans",
    format = "`[` vec($0, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct OrphansAttr(pub Vec<CoreOrphan>);

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreConvention {
    Imm,
    Mut,
    Var,
    Out,
    Ref,
    Deinit,
}

impl CoreConvention {
    pub const fn from_convention(convention: ArgConvention) -> Self {
        match convention {
            ArgConvention::Imm => Self::Imm,
            ArgConvention::Mut => Self::Mut,
            ArgConvention::Var => Self::Var,
            ArgConvention::Out => Self::Out,
            ArgConvention::Ref => Self::Ref,
            ArgConvention::Deinit => Self::Deinit,
        }
    }

    pub const fn convention(self) -> ArgConvention {
        match self {
            Self::Imm => ArgConvention::Imm,
            Self::Mut => ArgConvention::Mut,
            Self::Var => ArgConvention::Var,
            Self::Out => ArgConvention::Out,
            Self::Ref => ArgConvention::Ref,
            Self::Deinit => ArgConvention::Deinit,
        }
    }
}

/// The literal a parameter default folds to.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreLiteral {
    /// An exact integer in decimal, of any magnitude.
    Int(Text),
    /// An exact floating literal in its own spelling.
    Float(Text),
    Bool(bool),
    Str(Text),
    None,
    Dtype(super::types::CoreDtype),
}

/// A parameter default: a literal, under the converting constructors that
/// materialize it, outermost first.
#[format("`[` vec($constructors, CharSpace(`,`)) `] ` $literal")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreDefault {
    pub constructors: Vec<Text>,
    pub literal: CoreLiteral,
}

impl CoreDefault {
    pub fn from_const(constant: &CheckedConst) -> Result<Self, A1Error> {
        let mut constructors = Vec::new();
        let mut current = constant;
        while let CheckedConst::Construct { target, arg } = current {
            constructors.push(Text::from(target));
            current = arg;
        }
        let literal = match current {
            CheckedConst::Int(value) => CoreLiteral::Int(value.as_bigint().to_string().into()),
            CheckedConst::Float(value) => CoreLiteral::Float(value.to_string().into()),
            CheckedConst::Bool(value) => CoreLiteral::Bool(*value),
            CheckedConst::String(value) => CoreLiteral::Str(value.into()),
            CheckedConst::None => CoreLiteral::None,
            CheckedConst::Dtype(dtype) => {
                CoreLiteral::Dtype(super::types::CoreDtype::from_dtype(*dtype))
            }
            CheckedConst::Construct { .. } => {
                return Err(A1Error::new(
                    A1ErrorKind::UnsupportedForm,
                    "a conversion default without a literal has no core form",
                ));
            }
            CheckedConst::Evaluate { .. } => {
                return Err(A1Error::new(
                    A1ErrorKind::UnsupportedForm,
                    "an evaluated default has no core form",
                ));
            }
        };
        Ok(Self {
            constructors,
            literal,
        })
    }

    pub fn constant(&self) -> Result<CheckedConst, A1Error> {
        let literal = match &self.literal {
            CoreLiteral::Int(digits) => CheckedConst::Int(
                IntLiteral::parse_radix(digits.as_str(), 10).ok_or_else(|| {
                    A1Error::new(
                        A1ErrorKind::Export,
                        format!("`{digits}` is not a decimal integer literal"),
                    )
                })?,
            ),
            CoreLiteral::Float(text) => CheckedConst::Float(parse_float_literal(text)?),
            CoreLiteral::Bool(value) => CheckedConst::Bool(*value),
            CoreLiteral::Str(value) => CheckedConst::String(value.0.clone()),
            CoreLiteral::None => CheckedConst::None,
            CoreLiteral::Dtype(dtype) => CheckedConst::Dtype(dtype.dtype()),
        };
        Ok(self
            .constructors
            .iter()
            .rev()
            .fold(literal, |arg, target| CheckedConst::Construct {
                target: target.0.clone(),
                arg: Box::new(arg),
            }))
    }
}

#[format(
    "$name ` : ` $ty ` ` $required ` convention` opt($convention, delimiters(`(`, `)`)) ` ` $by_ref ` ` $writes ` default` opt($default, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreParam {
    pub name: Text,
    pub ty: TypeHandle,
    pub required: bool,
    pub convention: Option<CoreConvention>,
    pub by_ref: bool,
    pub writes: bool,
    pub default: Option<CoreDefault>,
}

/// A variadic collector of a declaration. Its three facts are recorded
/// independently, as the declaration table holds them.
#[format(
    "`type` opt($ty, delimiters(`(`, `)`)) ` convention` opt($convention, delimiters(`(`, `)`)) ` index` opt($index, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreCollector {
    pub ty: Option<TypeHandle>,
    pub convention: Option<CoreConvention>,
    pub index: Option<u64>,
}

impl CoreCollector {
    pub const fn is_absent(&self) -> bool {
        self.ty.is_none() && self.convention.is_none() && self.index.is_none()
    }
}

/// A callable's checked declaration, as the module's table records it.
#[pliron_attr(
    name = "mojito_core.declaration",
    format = "$symbol ` [` vec($params, CharSpace(`,`)) `] ` $has_receiver ` receiver` opt($receiver, delimiters(`(`, `)`)) ` -> ` $ret ` ` $returns_reference ` ` $raises ` error` opt($error, delimiters(`(`, `)`)) ` variadic` opt($variadic, delimiters(`(`, `)`)) ` kw_variadic` opt($kw_variadic, delimiters(`(`, `)`)) ` positional_only` opt($positional_only, delimiters(`(`, `)`)) ` keyword_only` opt($keyword_only, delimiters(`(`, `)`))",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct DeclarationAttr {
    pub symbol: Text,
    pub params: Vec<CoreParam>,
    pub has_receiver: bool,
    pub receiver: Option<CoreConvention>,
    pub ret: TypeHandle,
    pub returns_reference: bool,
    pub raises: bool,
    pub error: Option<TypeHandle>,
    pub variadic: Option<CoreCollector>,
    pub kw_variadic: Option<CoreCollector>,
    /// The count of leading positional-only parameters.
    pub positional_only: Option<u64>,
    /// The index of the first keyword-only parameter.
    pub keyword_only: Option<u64>,
}

#[format("$name ` : ` $ty")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreField {
    pub name: Text,
    pub ty: TypeHandle,
}

#[format("$name ` ` $raises")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreDestructor {
    pub name: Text,
    pub raises: bool,
}

#[format(
    "$name ` [` vec($fields, CharSpace(`,`)) `] mut [` vec($mut_self_methods, CharSpace(`,`)) `] ` $fieldwise_init ` message` opt($explicit_destroy_message, delimiters(`(`, `)`)) ` destructors [` vec($explicit_destructors, CharSpace(`,`)) `]`"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreStruct {
    pub name: Text,
    pub fields: Vec<CoreField>,
    /// Sorted, so the table has one spelling.
    pub mut_self_methods: Vec<Text>,
    pub fieldwise_init: bool,
    pub explicit_destroy_message: Option<Text>,
    /// Sorted by name.
    pub explicit_destructors: Vec<CoreDestructor>,
}

#[format("$requested ` -> ` $concrete")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreEntry {
    pub requested: Text,
    pub concrete: Text,
}

/// The module's tables: schema tag, ordered sources, entry map, and the
/// nominal declarations every core type and call resolves against.
#[pliron_attr(
    name = "mojito_core.module_tables",
    format = "$schema ` sources [` vec($sources, CharSpace(`,`)) `] entries [` vec($entries, CharSpace(`,`)) `] structs [` vec($structs, CharSpace(`,`)) `] declarations [` vec($declaration_order, CharSpace(`,`)) `]`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ModuleAttr {
    pub schema: Text,
    pub sources: Vec<Text>,
    pub entries: Vec<CoreEntry>,
    pub structs: Vec<CoreStruct>,
    /// The declaration table's order, which the function order need not
    /// follow.
    pub declaration_order: Vec<Text>,
}

/// The experimental schema tag; this is not `.mir` and `exec` never reads it.
pub const SCHEMA: &str = "mojito-a1-core 0";

/// The name a variable carries in diagnostics, by slot.
pub fn var_name(names: &[String], var: u32) -> Text {
    names.get(var as usize).map(Text::from).unwrap_or_default()
}
