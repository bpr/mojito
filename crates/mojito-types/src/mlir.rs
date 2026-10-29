//! The `__mlir_op` operations Mojito admits beyond the ownership markers.
//!
//! These are upstream's elementwise `pop` ops over `!kgen.simd` storage
//! (`pop.max` and `pop.min` among them, and `pop.cast` to and from `bool`
//! lanes), its `pop.bitcast` reinterpretation, its `pop.select` of one of two register-passable values, its `pop.simd`
//! lane ops (select, extract, insert, and the `bool`
//! reductions), the `index` ops over `__mlir_type.index`, and the
//! `pop.cast_from_builtin` and `pop.cast_to_builtin` casts between `index`
//! and its `!kgen.scalar<index>` storage and between `i1` and its
//! `!kgen.scalar<bool>` storage, with the
//! `__mlir_attr` spellings their attributes take. The checker verifies an op's operands
//! as upstream's op verifier does and records the [`MlirOp`]. MIR lowering
//! reads [`MlirOp::lowering`], and the backends compute a [`PopOp`] lane by
//! [`pop_int_lane`], [`pop_float_lane`], and [`pop_bool_lane`].

use mojito_ast::ast::{Dtype, InfixOp};

/// One admitted `pop` or `index` operation, named after upstream's
/// spelling; a compare carries its predicate attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MlirOp {
    PopAdd,
    PopSub,
    PopMul,
    PopDiv,
    PopFloorDiv,
    PopRem,
    PopNeg,
    PopAbs,
    PopShl,
    PopShr,
    PopCmp(CmpPredicate),
    PopCast,
    PopBitcast,
    PopSplat,
    PopAnd,
    PopOr,
    PopXor,
    PopRound,
    PopFloor,
    PopCeil,
    PopTrunc,
    PopFma,
    PopMax,
    PopMin,
    PopSelect,
    PopSimdSelect,
    PopSimdExtractElement,
    PopSimdInsertElement,
    PopSimdReduceAnd,
    PopSimdReduceOr,
    PopCastFromBuiltin,
    PopCastToBuiltin,
    IndexAdd,
    IndexSub,
    IndexMul,
    IndexDivs,
    IndexAnd,
    IndexShrs,
    IndexCmp(CmpPredicate),
    PopStringSize,
    PopStringAddress,
}

/// Why an `__mlir_op` spelling names no admitted operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MlirOpError {
    /// A name outside the admitted `pop` and `index` ops.
    Unknown,
    /// A compare without its `pred` attribute.
    MissingPredicate,
    /// A `pred` attribute on an op that takes none.
    UnexpectedPredicate,
    /// A `pred` attribute that is not the op's predicate kind, or names
    /// a predicate outside the admitted set.
    BadPredicate(String),
}

impl MlirOp {
    /// The operation spelled `name`, with the `pred` attribute's text
    /// (`#kgen.cmp_pred<lt>`, `#index.cmp_predicate<slt>`) when the call
    /// gave one.
    pub fn parse(name: &str, predicate: Option<&str>) -> Result<Self, MlirOpError> {
        let compare = |parse: fn(&str) -> Option<CmpPredicate>| {
            let attribute = predicate.ok_or(MlirOpError::MissingPredicate)?;
            parse(attribute).ok_or_else(|| MlirOpError::BadPredicate(attribute.to_string()))
        };
        let op = match name {
            "pop.cmp" => return compare(CmpPredicate::parse_kgen).map(Self::PopCmp),
            "index.cmp" => return compare(CmpPredicate::parse_index).map(Self::IndexCmp),
            "pop.add" => Self::PopAdd,
            "pop.sub" => Self::PopSub,
            "pop.mul" => Self::PopMul,
            "pop.div" => Self::PopDiv,
            "pop.floordiv" => Self::PopFloorDiv,
            "pop.rem" => Self::PopRem,
            "pop.neg" => Self::PopNeg,
            "pop.abs" => Self::PopAbs,
            "pop.shl" => Self::PopShl,
            "pop.shr" => Self::PopShr,
            "pop.cast" => Self::PopCast,
            "pop.bitcast" => Self::PopBitcast,
            "pop.simd.splat" => Self::PopSplat,
            "pop.simd.and" => Self::PopAnd,
            "pop.simd.or" => Self::PopOr,
            "pop.simd.xor" => Self::PopXor,
            "pop.round" => Self::PopRound,
            "pop.floor" => Self::PopFloor,
            "pop.ceil" => Self::PopCeil,
            "pop.trunc" => Self::PopTrunc,
            "pop.fma" => Self::PopFma,
            "pop.max" => Self::PopMax,
            "pop.min" => Self::PopMin,
            "pop.select" => Self::PopSelect,
            "pop.simd.select" => Self::PopSimdSelect,
            "pop.simd.extractelement" => Self::PopSimdExtractElement,
            "pop.simd.insertelement" => Self::PopSimdInsertElement,
            "pop.simd.reduce_and" => Self::PopSimdReduceAnd,
            "pop.simd.reduce_or" => Self::PopSimdReduceOr,
            "pop.cast_from_builtin" => Self::PopCastFromBuiltin,
            "pop.cast_to_builtin" => Self::PopCastToBuiltin,
            "pop.string.size" => Self::PopStringSize,
            "pop.string.address" => Self::PopStringAddress,
            "index.add" => Self::IndexAdd,
            "index.sub" => Self::IndexSub,
            "index.mul" => Self::IndexMul,
            "index.divs" => Self::IndexDivs,
            "index.and" => Self::IndexAnd,
            "index.shrs" => Self::IndexShrs,
            _ => return Err(MlirOpError::Unknown),
        };
        match predicate {
            Some(_) => Err(MlirOpError::UnexpectedPredicate),
            None => Ok(op),
        }
    }

    /// The upstream spelling of this operation.
    pub const fn name(self) -> &'static str {
        match self {
            Self::PopAdd => "pop.add",
            Self::PopSub => "pop.sub",
            Self::PopMul => "pop.mul",
            Self::PopDiv => "pop.div",
            Self::PopFloorDiv => "pop.floordiv",
            Self::PopRem => "pop.rem",
            Self::PopNeg => "pop.neg",
            Self::PopAbs => "pop.abs",
            Self::PopShl => "pop.shl",
            Self::PopShr => "pop.shr",
            Self::PopCmp(_) => "pop.cmp",
            Self::PopCast => "pop.cast",
            Self::PopBitcast => "pop.bitcast",
            Self::PopSplat => "pop.simd.splat",
            Self::PopAnd => "pop.simd.and",
            Self::PopOr => "pop.simd.or",
            Self::PopXor => "pop.simd.xor",
            Self::PopRound => "pop.round",
            Self::PopFloor => "pop.floor",
            Self::PopCeil => "pop.ceil",
            Self::PopTrunc => "pop.trunc",
            Self::PopFma => "pop.fma",
            Self::PopMax => "pop.max",
            Self::PopMin => "pop.min",
            Self::PopSelect => "pop.select",
            Self::PopSimdSelect => "pop.simd.select",
            Self::PopSimdExtractElement => "pop.simd.extractelement",
            Self::PopSimdInsertElement => "pop.simd.insertelement",
            Self::PopSimdReduceAnd => "pop.simd.reduce_and",
            Self::PopSimdReduceOr => "pop.simd.reduce_or",
            Self::PopCastFromBuiltin => "pop.cast_from_builtin",
            Self::PopCastToBuiltin => "pop.cast_to_builtin",
            Self::IndexAdd => "index.add",
            Self::IndexSub => "index.sub",
            Self::IndexMul => "index.mul",
            Self::IndexDivs => "index.divs",
            Self::IndexAnd => "index.and",
            Self::IndexShrs => "index.shrs",
            Self::IndexCmp(_) => "index.cmp",
            Self::PopStringSize => "pop.string.size",
            Self::PopStringAddress => "pop.string.address",
        }
    }

    /// Whether this is an `index` op, whose operands are
    /// `__mlir_type.index` values rather than `!kgen.simd` storage.
    pub const fn is_index(self) -> bool {
        matches!(
            self,
            Self::IndexAdd
                | Self::IndexSub
                | Self::IndexMul
                | Self::IndexDivs
                | Self::IndexAnd
                | Self::IndexShrs
                | Self::IndexCmp(_)
        )
    }

    /// Whether this is a `pop.simd` lane op, whose operands are not all of
    /// one vector type: a `bool` mask, a lane, or an `index` position.
    pub const fn is_lane(self) -> bool {
        matches!(
            self.lowering(),
            MlirLowering::SimdSelect
                | MlirLowering::Extract
                | MlirLowering::Insert
                | MlirLowering::Reduce(_)
        )
    }

    /// Whether this is `pop.cast_from_builtin` or `pop.cast_to_builtin`,
    /// which convert between an MLIR builtin type and its `!kgen.scalar`
    /// storage.
    pub const fn is_builtin_cast(self) -> bool {
        matches!(self, Self::PopCastFromBuiltin | Self::PopCastToBuiltin)
    }

    /// Whether the op names its result type with a `_type` attribute.
    pub const fn takes_result_type(self) -> bool {
        matches!(
            self,
            Self::PopCast
                | Self::PopBitcast
                | Self::PopSplat
                | Self::PopCastFromBuiltin
                | Self::PopCastToBuiltin
        )
    }

    /// The number of operands the op takes.
    pub const fn arity(self) -> usize {
        match self.lowering() {
            MlirLowering::Binary(_) => 2,
            MlirLowering::Negate
            | MlirLowering::Splat
            | MlirLowering::Cast
            | MlirLowering::Bitcast
            | MlirLowering::BuiltinCast
            | MlirLowering::Reduce(_) => 1,
            MlirLowering::Extract => 2,
            MlirLowering::Select | MlirLowering::SimdSelect | MlirLowering::Insert => 3,
            MlirLowering::Pop(op) => op.arity(),
        }
    }

    /// The dtypes an operand's lanes may hold, as upstream's verifier
    /// states them: `None` for an op whose lanes are unconstrained (a
    /// compare, a cast, a splat, or an `index` op).
    pub const fn lanes(self) -> Option<LaneKinds> {
        Some(match self {
            Self::PopShl | Self::PopShr => LaneKinds::Integral,
            Self::PopMax | Self::PopMin => LaneKinds::NumericOrBool,
            Self::PopAnd | Self::PopOr | Self::PopXor => LaneKinds::IntegralOrBool,
            Self::PopRound
            | Self::PopFloor
            | Self::PopCeil
            | Self::PopTrunc
            | Self::PopFma
            | Self::PopAdd
            | Self::PopSub
            | Self::PopMul
            | Self::PopDiv
            | Self::PopFloorDiv
            | Self::PopRem
            | Self::PopNeg
            | Self::PopAbs => LaneKinds::Numeric,
            _ => return None,
        })
    }

    /// The MIR instruction that computes this op.
    pub const fn lowering(self) -> MlirLowering {
        match self {
            Self::PopAdd | Self::IndexAdd => MlirLowering::Binary(InfixOp::Add),
            Self::PopSub | Self::IndexSub => MlirLowering::Binary(InfixOp::Sub),
            Self::PopMul | Self::IndexMul => MlirLowering::Binary(InfixOp::Mul),
            Self::PopFloorDiv => MlirLowering::Binary(InfixOp::FloorDiv),
            Self::PopShl => MlirLowering::Binary(InfixOp::Shl),
            Self::PopShr | Self::IndexShrs => MlirLowering::Binary(InfixOp::Shr),
            Self::PopAnd | Self::IndexAnd => MlirLowering::Binary(InfixOp::BitAnd),
            Self::PopOr => MlirLowering::Binary(InfixOp::BitOr),
            Self::PopXor => MlirLowering::Binary(InfixOp::BitXor),
            Self::PopCmp(predicate) | Self::IndexCmp(predicate) => predicate.lowering(),
            Self::PopNeg => MlirLowering::Negate,
            Self::PopSplat => MlirLowering::Splat,
            Self::PopCast => MlirLowering::Cast,
            Self::PopBitcast => MlirLowering::Bitcast,
            Self::PopCastFromBuiltin | Self::PopCastToBuiltin => MlirLowering::BuiltinCast,
            Self::PopDiv | Self::IndexDivs => MlirLowering::Pop(PopOp::Div),
            Self::PopRem => MlirLowering::Pop(PopOp::Rem),
            Self::PopAbs => MlirLowering::Pop(PopOp::Abs),
            Self::PopRound => MlirLowering::Pop(PopOp::Round),
            Self::PopFloor => MlirLowering::Pop(PopOp::Floor),
            Self::PopCeil => MlirLowering::Pop(PopOp::Ceil),
            Self::PopTrunc => MlirLowering::Pop(PopOp::Trunc),
            Self::PopFma => MlirLowering::Pop(PopOp::Fma),
            Self::PopMax => MlirLowering::Pop(PopOp::Max),
            Self::PopMin => MlirLowering::Pop(PopOp::Min),
            Self::PopStringSize => MlirLowering::Pop(PopOp::StringSize),
            Self::PopStringAddress => MlirLowering::Pop(PopOp::StringAddress),
            Self::PopSelect => MlirLowering::Select,
            Self::PopSimdSelect => MlirLowering::SimdSelect,
            Self::PopSimdExtractElement => MlirLowering::Extract,
            Self::PopSimdInsertElement => MlirLowering::Insert,
            Self::PopSimdReduceAnd => MlirLowering::Reduce(SimdReduction::And),
            Self::PopSimdReduceOr => MlirLowering::Reduce(SimdReduction::Or),
        }
    }
}

/// The lane dtypes an op's verifier admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneKinds {
    /// Integer or float lanes.
    Numeric,
    /// Integer lanes.
    Integral,
    /// Integer or `bool` lanes.
    IntegralOrBool,
    /// Integer, float, or `bool` lanes.
    NumericOrBool,
}

impl LaneKinds {
    /// Whether a lane of `dtype` is admitted.
    pub const fn admits(self, dtype: Dtype) -> bool {
        match self {
            Self::Numeric => !matches!(dtype, Dtype::Bool),
            Self::Integral => !matches!(dtype, Dtype::Bool) && !dtype.is_float(),
            Self::IntegralOrBool => !dtype.is_float(),
            Self::NumericOrBool => true,
        }
    }

    /// How upstream's verifier names the admitted lanes.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Numeric => "integer or float",
            Self::Integral => "integral",
            Self::IntegralOrBool => "integral or bool",
            Self::NumericOrBool => "integer, float, or bool",
        }
    }
}

/// The bits `!kgen.simd` storage of `width` `dtype` lanes occupies, as
/// `pop.bitcast` counts them: a `bool` lane is one bit, and `Int` and
/// `UInt` lanes are 64. A bitcast keeps this total.
pub fn storage_bits(dtype: Dtype, width: i64) -> i64 {
    i64::from(dtype.bit_width()) * width
}

/// The instruction an [`MlirOp`] lowers to.
///
/// That is the built-in operator whose lane semantics are the op's
/// (`binary`/`unary`), a construction, cast, or bitcast carrying the
/// `_type` attribute's slots, the `pop` instruction for an op no operator
/// computes, the `select` of one of two register values, the
/// `simd.select`, `simd.extract`, `simd.insert`, or
/// `simd.reduce` lane instruction, or the `builtin.cast` that retypes an
/// `index` as its `!kgen.scalar<index>` storage or back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlirLowering {
    Binary(InfixOp),
    Negate,
    Splat,
    Cast,
    Bitcast,
    BuiltinCast,
    Pop(PopOp),
    Select,
    SimdSelect,
    Extract,
    Insert,
    Reduce(SimdReduction),
}

/// A lane reduction over integer or `bool` lanes: `pop.simd.reduce_and`
/// (the lanes' bitwise and; every `bool` lane is set) or
/// `pop.simd.reduce_or` (their bitwise or; some `bool` lane is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SimdReduction {
    And,
    Or,
}

impl SimdReduction {
    /// The reduction's name after `pop.simd.reduce_`, its MIR text
    /// spelling.
    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// The reduction spelled `mnemonic`.
    pub fn from_mnemonic(mnemonic: &str) -> Option<Self> {
        [Self::And, Self::Or]
            .into_iter()
            .find(|reduction| reduction.mnemonic() == mnemonic)
    }

    /// The reduction of `bool` lanes.
    pub fn reduce_bools(self, mut lanes: impl Iterator<Item = bool>) -> bool {
        match self {
            Self::And => lanes.all(|lane| lane),
            Self::Or => lanes.any(|lane| lane),
        }
    }

    /// The reduction of integer lanes, each held sign- or zero-extended:
    /// the bitwise fold keeps that extension.
    pub fn reduce_ints(self, lanes: impl Iterator<Item = i128>) -> i128 {
        match self {
            Self::And => lanes.fold(-1, |acc, lane| acc & lane),
            Self::Or => lanes.fold(0, |acc, lane| acc | lane),
        }
    }
}

/// A compare predicate of `pop.cmp` or `index.cmp`.
///
/// `#kgen.cmp_pred<…>`'s ordering follows the lanes' signedness;
/// `#index.cmp_predicate<…>` names a signed (`slt`, as `Lt`) or unsigned
/// (`ult`) ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CmpPredicate {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Ult,
    Ule,
    Ugt,
    Uge,
}

impl CmpPredicate {
    /// The predicate of `#kgen.cmp_pred<p>`.
    pub fn parse_kgen(attribute: &str) -> Option<Self> {
        let name = attribute
            .strip_prefix("#kgen.cmp_pred<")?
            .strip_suffix('>')?;
        Self::named(name, ["eq", "ne", "lt", "le", "gt", "ge"])
    }

    /// The predicate of `#index.cmp_predicate<p>`: equality, or a signed or
    /// unsigned ordering.
    pub fn parse_index(attribute: &str) -> Option<Self> {
        let name = attribute
            .strip_prefix("#index.cmp_predicate<")?
            .strip_suffix('>')?;
        Self::named(name, ["eq", "ne", "slt", "sle", "sgt", "sge"]).or_else(|| {
            [
                ("ult", Self::Ult),
                ("ule", Self::Ule),
                ("ugt", Self::Ugt),
                ("uge", Self::Uge),
            ]
            .into_iter()
            .find_map(|(spelling, predicate)| (spelling == name).then_some(predicate))
        })
    }

    /// The instruction computing this compare: the infix compare, whose
    /// ordering is the operands' own, or the `pop` instruction's unsigned
    /// compare.
    pub const fn lowering(self) -> MlirLowering {
        MlirLowering::Binary(match self {
            Self::Eq => InfixOp::Eq,
            Self::Ne => InfixOp::Ne,
            Self::Lt => InfixOp::Lt,
            Self::Le => InfixOp::Le,
            Self::Gt => InfixOp::Gt,
            Self::Ge => InfixOp::Ge,
            Self::Ult => return MlirLowering::Pop(PopOp::Ult),
            Self::Ule => return MlirLowering::Pop(PopOp::Ule),
            Self::Ugt => return MlirLowering::Pop(PopOp::Ugt),
            Self::Uge => return MlirLowering::Pop(PopOp::Uge),
        })
    }

    fn named(name: &str, spellings: [&str; 6]) -> Option<Self> {
        let all = [Self::Eq, Self::Ne, Self::Lt, Self::Le, Self::Gt, Self::Ge];
        spellings
            .iter()
            .position(|spelling| *spelling == name)
            .map(|index| all[index])
    }
}

/// The operations the MIR `pop` instruction computes.
///
/// These are the `pop` lane ops whose semantics no built-in operator has
/// (`div` and `rem` truncate, `abs` wraps, the rounding ops, `fma`, and
/// `max` and `min`), `index.cmp`'s unsigned compares, and the reads of a
/// run-time `!kgen.string`: `pop.string.size`, its byte length as an
/// `index`, and `pop.string.address`, the address of its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PopOp {
    Div,
    Rem,
    Abs,
    Round,
    Floor,
    Ceil,
    Trunc,
    Fma,
    Max,
    Min,
    Ult,
    Ule,
    Ugt,
    Uge,
    StringSize,
    StringAddress,
}

impl PopOp {
    /// Every operation, in declaration order.
    pub const ALL: [Self; 16] = [
        Self::Div,
        Self::Rem,
        Self::Abs,
        Self::Round,
        Self::Floor,
        Self::Ceil,
        Self::Trunc,
        Self::Fma,
        Self::Max,
        Self::Min,
        Self::Ult,
        Self::Ule,
        Self::Ugt,
        Self::Uge,
        Self::StringSize,
        Self::StringAddress,
    ];

    /// The number of operands.
    pub const fn arity(self) -> usize {
        match self {
            Self::Abs
            | Self::Round
            | Self::Floor
            | Self::Ceil
            | Self::Trunc
            | Self::StringSize
            | Self::StringAddress => 1,
            Self::Fma => 3,
            _ => 2,
        }
    }

    /// Whether this is an unsigned compare, whose result is an `i1`
    /// (`Bool`) rather than its operands' type.
    pub const fn is_compare(self) -> bool {
        matches!(self, Self::Ult | Self::Ule | Self::Ugt | Self::Uge)
    }

    /// Whether this reads a `!kgen.string` operand, whose result is an
    /// `index` or a pointer rather than lanes of the operand's type.
    pub const fn is_string_read(self) -> bool {
        matches!(self, Self::StringSize | Self::StringAddress)
    }

    /// The op's name after `pop.`, its MIR text spelling.
    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::Div => "div",
            Self::Rem => "rem",
            Self::Abs => "abs",
            Self::Round => "round",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Trunc => "trunc",
            Self::Fma => "fma",
            Self::Max => "max",
            Self::Min => "min",
            Self::Ult => "ult",
            Self::Ule => "ule",
            Self::Ugt => "ugt",
            Self::Uge => "uge",
            Self::StringSize => "string.size",
            Self::StringAddress => "string.address",
        }
    }

    /// The operation spelled `mnemonic`.
    pub fn from_mnemonic(mnemonic: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.mnemonic() == mnemonic)
    }
}

/// Whether `attribute` is a `#pop.fmf<flag>` fast-math attribute.
///
/// The flags permit, never require, a relaxed result, so exact arithmetic
/// implements every one of them: the check reads a closed flag, spelled or
/// computed (`flag._mlir_attr()`), and drops it before MIR.
pub fn is_fastmath_attribute(attribute: &str) -> bool {
    attribute
        .strip_prefix("#pop.fmf<")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|flag| {
            matches!(
                flag,
                "none" | "fast" | "nnan" | "ninf" | "nsz" | "arcp" | "contract" | "afn" | "reassoc"
            )
        })
}

/// One integer lane of `op` over lanes of `dtype`, each held sign- or
/// zero-extended, as upstream's `pop` op computes it, or `None` for a zero
/// divisor (undefined upstream).
///
/// `div` and `rem` truncate toward zero, `abs` wraps (`int8 -128` stays
/// `-128`) and is the identity on an unsigned lane, the rounding ops are
/// the identity, `fma` wraps `a * b + c`, `max` and `min` order the lanes
/// by their signedness, and an unsigned compare reads both lanes'
/// `dtype`-wide bits unsigned and gives 1 or 0.
pub fn pop_int_lane(op: PopOp, dtype: Dtype, lanes: &[i128]) -> Option<i128> {
    let unsigned = |lane: i128| lane & ((1i128 << dtype.bit_width()) - 1);
    let lane = match (op, lanes) {
        (PopOp::Div | PopOp::Rem, [_, 0]) => return None,
        (PopOp::Div, [a, b]) => a.wrapping_div(*b),
        (PopOp::Rem, [a, b]) => a.wrapping_rem(*b),
        (PopOp::Abs, [a]) => a.wrapping_abs(),
        (PopOp::Round | PopOp::Floor | PopOp::Ceil | PopOp::Trunc, [a]) => *a,
        (PopOp::Fma, [a, b, c]) => a.wrapping_mul(*b).wrapping_add(*c),
        (PopOp::Max, [a, b]) => *a.max(b),
        (PopOp::Min, [a, b]) => *a.min(b),
        (PopOp::Ult, [a, b]) => return Some(i128::from(unsigned(*a) < unsigned(*b))),
        (PopOp::Ule, [a, b]) => return Some(i128::from(unsigned(*a) <= unsigned(*b))),
        (PopOp::Ugt, [a, b]) => return Some(i128::from(unsigned(*a) > unsigned(*b))),
        (PopOp::Uge, [a, b]) => return Some(i128::from(unsigned(*a) >= unsigned(*b))),
        _ => return None,
    };
    Some(crate::ct::wrap_lane(dtype, lane))
}

/// One `bool` lane of `op`: `max` is the lanes' or and `min` their and,
/// as over `i1`; `None` for an op `bool` lanes do not take.
pub const fn pop_bool_lane(op: PopOp, a: bool, b: bool) -> Option<bool> {
    match op {
        PopOp::Max => Some(a | b),
        PopOp::Min => Some(a & b),
        _ => None,
    }
}

/// One float lane of `op` over lanes of `dtype`, computed at double
/// precision and rounded once to the lane.
///
/// `rem` is the truncating remainder (`fmod`, exact), `round` rounds half
/// to even, `fma` rounds the fused product-sum once (a `float32` lane
/// fuses at single precision, as `llvm.fma.f32` does), and `max` and `min`
/// are `llvm.maxnum` and `llvm.minnum`: a NaN lane gives the other one,
/// and equal lanes (`0.0` and `-0.0`) give the first.
pub fn pop_float_lane(op: PopOp, dtype: Dtype, lanes: &[f64]) -> Option<f64> {
    let lane = match (op, lanes) {
        (PopOp::Div, [a, b]) => a / b,
        (PopOp::Rem, [a, b]) => a % b,
        (PopOp::Abs, [a]) => a.abs(),
        (PopOp::Round, [a]) => a.round_ties_even(),
        (PopOp::Floor, [a]) => a.floor(),
        (PopOp::Ceil, [a]) => a.ceil(),
        (PopOp::Trunc, [a]) => a.trunc(),
        (PopOp::Fma, [a, b, c]) if dtype == Dtype::Float32 => {
            f64::from((*a as f32).mul_add(*b as f32, *c as f32))
        }
        (PopOp::Fma, [a, b, c]) => a.mul_add(*b, *c),
        (PopOp::Max, [a, b]) if a.is_nan() || b > a => *b,
        (PopOp::Min, [a, b]) if a.is_nan() || b < a => *b,
        (PopOp::Max | PopOp::Min, [a, _]) => *a,
        _ => return None,
    };
    Some(dtype.round_lane(lane))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_spellings_parse() {
        for name in [
            "pop.add",
            "pop.sub",
            "pop.mul",
            "pop.div",
            "pop.floordiv",
            "pop.rem",
            "pop.neg",
            "pop.abs",
            "pop.shl",
            "pop.shr",
            "pop.cast",
            "pop.simd.splat",
            "pop.simd.and",
            "pop.simd.or",
            "pop.simd.xor",
            "pop.round",
            "pop.floor",
            "pop.ceil",
            "pop.trunc",
            "pop.fma",
            "pop.max",
            "pop.min",
            "pop.simd.select",
            "pop.simd.extractelement",
            "pop.simd.insertelement",
            "pop.simd.reduce_and",
            "pop.simd.reduce_or",
            "pop.cast_from_builtin",
            "pop.cast_to_builtin",
            "index.add",
            "index.sub",
            "index.mul",
            "index.divs",
            "index.and",
            "index.shrs",
        ] {
            let op = MlirOp::parse(name, None).expect(name);
            assert_eq!(op.name(), name);
        }
        for predicate in ["eq", "ne", "lt", "le", "gt", "ge"] {
            let attribute = format!("#kgen.cmp_pred<{predicate}>");
            assert!(matches!(
                MlirOp::parse("pop.cmp", Some(&attribute)),
                Ok(MlirOp::PopCmp(_))
            ));
        }
        for predicate in [
            "eq", "ne", "slt", "sle", "sgt", "sge", "ult", "ule", "ugt", "uge",
        ] {
            let attribute = format!("#index.cmp_predicate<{predicate}>");
            assert!(matches!(
                MlirOp::parse("index.cmp", Some(&attribute)),
                Ok(MlirOp::IndexCmp(_))
            ));
        }
        assert!(is_fastmath_attribute("#pop.fmf<fast>"));
        assert!(is_fastmath_attribute("#pop.fmf<contract>"));
    }

    #[test]
    fn spellings_outside_the_subset_do_not_parse() {
        assert_eq!(
            MlirOp::parse("pop.simd.shuffle", None),
            Err(MlirOpError::Unknown)
        );
        assert!(matches!(
            MlirOp::parse("pop.cmp", Some("#kgen.cmp_pred<ult>")),
            Err(MlirOpError::BadPredicate(_))
        ));
        assert_eq!(
            MlirOp::parse("pop.cmp", None),
            Err(MlirOpError::MissingPredicate)
        );
        assert!(matches!(
            MlirOp::parse("pop.cmp", Some("#index.cmp_predicate<slt>")),
            Err(MlirOpError::BadPredicate(_))
        ));
        assert_eq!(
            MlirOp::parse("pop.add", Some("#kgen.cmp_pred<lt>")),
            Err(MlirOpError::UnexpectedPredicate)
        );
        assert!(!is_fastmath_attribute("#pop.fmf<bogus>"));
        assert!(!is_fastmath_attribute("#pop.fmf<nnan, ninf>"));
    }

    #[test]
    fn lanes_follow_the_pin() {
        assert_eq!(pop_int_lane(PopOp::Div, Dtype::Int, &[-7, 2]), Some(-3));
        assert_eq!(pop_int_lane(PopOp::Rem, Dtype::Int, &[-7, 2]), Some(-1));
        assert_eq!(pop_int_lane(PopOp::Div, Dtype::UInt8, &[250, 3]), Some(83));
        assert_eq!(pop_int_lane(PopOp::Abs, Dtype::Int8, &[-128]), Some(-128));
        assert_eq!(pop_int_lane(PopOp::Div, Dtype::Int, &[1, 0]), None);
        assert_eq!(
            pop_float_lane(PopOp::Rem, Dtype::Float32, &[-7.5, 2.0]),
            Some(-1.5)
        );
        assert_eq!(
            pop_float_lane(PopOp::Round, Dtype::Float64, &[2.5]),
            Some(2.0)
        );
        assert_eq!(
            pop_float_lane(PopOp::Round, Dtype::Float64, &[-2.5]),
            Some(-2.0)
        );
        assert_eq!(
            pop_int_lane(PopOp::Max, Dtype::UInt8, &[200, 100]),
            Some(200)
        );
        assert_eq!(
            pop_int_lane(PopOp::Min, Dtype::Int8, &[-100, 100]),
            Some(-100)
        );
        assert_eq!(pop_int_lane(PopOp::Ult, Dtype::Int, &[-1, 3]), Some(0));
        assert_eq!(pop_int_lane(PopOp::Ugt, Dtype::Int, &[-1, 3]), Some(1));
        assert_eq!(
            pop_float_lane(PopOp::Max, Dtype::Float64, &[f64::NAN, 1.0]),
            Some(1.0)
        );
        assert_eq!(
            pop_float_lane(PopOp::Min, Dtype::Float64, &[2.0, f64::NAN]),
            Some(2.0)
        );
        assert_eq!(pop_bool_lane(PopOp::Max, true, false), Some(true));
    }
}
