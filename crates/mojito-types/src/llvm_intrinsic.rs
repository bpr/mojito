//! The LLVM intrinsics `pop.call_llvm_intrinsic` reaches in Mojito.
//!
//! Upstream's `std.sys.intrinsics.llvm_intrinsic[name, T]` calls any LLVM
//! intrinsic by name, and LLVM translation rejects a name it does not know
//! or a signature the intrinsic does not take. Mojito computes a table of
//! them: the checker rejects a closed name outside it, the elaborator a
//! reached instance's, and the concrete verifier a signature
//! [`LlvmIntrinsic::signature_error`] refuses. A shuffle's index operand
//! ([`LlvmIntrinsic::index_operand`]) must hold a value
//! [`LlvmIntrinsic::index_error`] admits. The VM computes a lane by
//! [`int_lane`], [`float_lane`], [`reduce_ints`], [`reduce_floats`],
//! [`LlvmIntrinsic::shuffle_sources`], and [`step_lanes`], and the native
//! backend calls the intrinsic, mangled by [`LlvmIntrinsic::overloads`].

use crate::types::{SimdDtype, SimdWidth, Ty, TyKind, simd_slots};
use mojito_ast::ast::Dtype;

/// One LLVM intrinsic in the table, named after its LLVM spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LlvmIntrinsic {
    Ctlz,
    Cttz,
    Ctpop,
    Bitreverse,
    Bswap,
    Fshl,
    Fshr,
    Copysign,
    Sqrt,
    Donothing,
    VectorReduce(VectorReduction),
    VectorExtract,
    VectorInsert,
    VectorSplice(SpliceDirection),
    VectorInterleave2,
    VectorDeinterleave2,
    Stepvector,
}

/// The end of `llvm.vector.splice.left`/`right`'s operands' concatenation
/// its offset counts from: the left one's first lane, or the right one's
/// last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpliceDirection {
    Left,
    Right,
}

/// An `llvm.vector.reduce.*` reduction of a vector to one lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VectorReduction {
    And,
    Or,
    Smax,
    Smin,
    Umax,
    Umin,
    Fmax,
    Fmin,
}

/// A type an overloaded intrinsic's name is mangled by: its result's or
/// an operand's (`llvm.ctpop.v4i32`, `llvm.vector.reduce.smax.v4i32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadSlot {
    Result,
    Operand(usize),
}

impl LlvmIntrinsic {
    /// Every intrinsic in the table.
    pub const ALL: [Self; 25] = [
        Self::Ctlz,
        Self::Cttz,
        Self::Ctpop,
        Self::Bitreverse,
        Self::Bswap,
        Self::Fshl,
        Self::Fshr,
        Self::Copysign,
        Self::Sqrt,
        Self::Donothing,
        Self::VectorReduce(VectorReduction::And),
        Self::VectorReduce(VectorReduction::Or),
        Self::VectorReduce(VectorReduction::Smax),
        Self::VectorReduce(VectorReduction::Smin),
        Self::VectorReduce(VectorReduction::Umax),
        Self::VectorReduce(VectorReduction::Umin),
        Self::VectorReduce(VectorReduction::Fmax),
        Self::VectorReduce(VectorReduction::Fmin),
        Self::VectorExtract,
        Self::VectorInsert,
        Self::VectorSplice(SpliceDirection::Left),
        Self::VectorSplice(SpliceDirection::Right),
        Self::VectorInterleave2,
        Self::VectorDeinterleave2,
        Self::Stepvector,
    ];

    /// The intrinsic LLVM spells `name`, if it is in the table.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|intrinsic| intrinsic.name() == name)
    }

    /// The intrinsic's LLVM spelling, before overload mangling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ctlz => "llvm.ctlz",
            Self::Cttz => "llvm.cttz",
            Self::Ctpop => "llvm.ctpop",
            Self::Bitreverse => "llvm.bitreverse",
            Self::Bswap => "llvm.bswap",
            Self::Fshl => "llvm.fshl",
            Self::Fshr => "llvm.fshr",
            Self::Copysign => "llvm.copysign",
            Self::Sqrt => "llvm.sqrt",
            Self::Donothing => "llvm.donothing",
            Self::VectorReduce(VectorReduction::And) => "llvm.vector.reduce.and",
            Self::VectorReduce(VectorReduction::Or) => "llvm.vector.reduce.or",
            Self::VectorReduce(VectorReduction::Smax) => "llvm.vector.reduce.smax",
            Self::VectorReduce(VectorReduction::Smin) => "llvm.vector.reduce.smin",
            Self::VectorReduce(VectorReduction::Umax) => "llvm.vector.reduce.umax",
            Self::VectorReduce(VectorReduction::Umin) => "llvm.vector.reduce.umin",
            Self::VectorReduce(VectorReduction::Fmax) => "llvm.vector.reduce.fmax",
            Self::VectorReduce(VectorReduction::Fmin) => "llvm.vector.reduce.fmin",
            Self::VectorExtract => "llvm.vector.extract",
            Self::VectorInsert => "llvm.vector.insert",
            Self::VectorSplice(SpliceDirection::Left) => "llvm.vector.splice.left",
            Self::VectorSplice(SpliceDirection::Right) => "llvm.vector.splice.right",
            Self::VectorInterleave2 => "llvm.vector.interleave2",
            Self::VectorDeinterleave2 => "llvm.vector.deinterleave2",
            Self::Stepvector => "llvm.stepvector",
        }
    }

    /// The names in the table, for a diagnostic.
    pub fn admitted_names() -> String {
        Self::ALL
            .iter()
            .map(|intrinsic| format!("'{}'", intrinsic.name()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The number of operands the intrinsic takes.
    pub const fn arity(self) -> usize {
        match self {
            Self::Donothing | Self::Stepvector => 0,
            Self::Ctpop
            | Self::Bitreverse
            | Self::Bswap
            | Self::Sqrt
            | Self::VectorReduce(_)
            | Self::VectorDeinterleave2 => 1,
            Self::Ctlz
            | Self::Cttz
            | Self::Copysign
            | Self::VectorExtract
            | Self::VectorInterleave2 => 2,
            Self::Fshl | Self::Fshr | Self::VectorInsert | Self::VectorSplice(_) => 3,
        }
    }

    /// The types the intrinsic's LLVM name is mangled by, in order.
    pub const fn overloads(self) -> &'static [OverloadSlot] {
        match self {
            Self::Donothing => &[],
            Self::VectorReduce(_) | Self::VectorDeinterleave2 => &[OverloadSlot::Operand(0)],
            Self::VectorExtract => &[OverloadSlot::Result, OverloadSlot::Operand(0)],
            Self::VectorInsert => &[OverloadSlot::Result, OverloadSlot::Operand(1)],
            _ => &[OverloadSlot::Result],
        }
    }

    /// The number of elements of the literal struct LLVM gives as the
    /// intrinsic's result: two, a vector of the even lanes and one of the
    /// odd, for `vector.deinterleave2`, and one, the result itself, for
    /// every other intrinsic.
    pub const fn result_elements(self) -> usize {
        match self {
            Self::VectorDeinterleave2 => 2,
            _ => 1,
        }
    }

    /// The position of the one-lane integer operand a shuffle reads as a
    /// lane index rather than as lanes, with the bit width LLVM declares
    /// for it: the `i64` index of `vector.extract` and `vector.insert`,
    /// and the `i32` offset of a splice.
    pub const fn index_operand(self) -> Option<(usize, u32)> {
        match self {
            Self::VectorExtract => Some((1, 64)),
            Self::VectorInsert => Some((2, 64)),
            Self::VectorSplice(_) => Some((2, 32)),
            _ => None,
        }
    }

    /// Whether the [`Self::index_operand`] is an `immarg`, which LLVM
    /// translation requires a constant: an extract's or an insert's index,
    /// but not a splice's offset.
    pub const fn index_is_immediate(self) -> bool {
        matches!(self, Self::VectorExtract | Self::VectorInsert)
    }

    /// Why LLVM translation rejects a call whose [`Self::index_operand`]
    /// is `index`, its result `result_width` lanes wide and its vector
    /// operands `widths`, or `None` when it accepts it. An extract's or an
    /// insert's index must be a multiple of the subvector's width that
    /// keeps the subvector inside the vector, as LLVM's verifier requires;
    /// a splice offset past the vector is a poison result, not an error.
    pub fn index_error(self, result_width: usize, widths: &[usize], index: i128) -> Option<String> {
        let index = index as u64;
        let fits = |vector: usize, subvector: usize| {
            usize::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(subvector))
                .is_some_and(|end| end <= vector)
        };
        let (vector, subvector, misaligned, overrun) = match (self, widths) {
            (Self::VectorExtract, [vector]) => (
                *vector,
                result_width,
                "vector_extract index must be a constant multiple of the result type's known \
                 minimum vector length.",
                "vector_extract would overrun.",
            ),
            (Self::VectorInsert, [vector, subvector]) => (
                *vector,
                *subvector,
                "vector_insert index must be a constant multiple of the subvector's known \
                 minimum vector length.",
                "subvector operand of vector_insert would overrun the vector being inserted \
                 into.",
            ),
            _ => return None,
        };
        let reason = if index.is_multiple_of(subvector as u64) {
            (!fits(vector, subvector)).then_some(overrun)
        } else {
            Some(misaligned)
        };
        reason.map(str::to_string)
    }

    /// Where each lane of a shuffling intrinsic's `result_width`-lane
    /// result comes from: the position of a lane among its vector
    /// operands' lanes laid end to end (operands `widths` wide), or `None`
    /// for a lane a splice offset past the vector leaves poison, which
    /// Mojito computes as zero. `vector.deinterleave2`'s result is its
    /// [`Self::result_elements`], each `result_width` lanes, laid end to
    /// end. `index` is the [`Self::index_operand`]'s value; `None` is an
    /// intrinsic that does not shuffle.
    pub fn shuffle_sources(
        self,
        result_width: usize,
        widths: &[usize],
        index: i128,
    ) -> Option<Vec<Option<usize>>> {
        let offset = |lane: usize| {
            i128::try_from(lane)
                .ok()
                .and_then(|lane| lane.checked_add(index))
                .and_then(|source| usize::try_from(source).ok())
        };
        let sources = match (self, widths) {
            (Self::VectorExtract, [_]) => (0..result_width).map(offset).collect(),
            (Self::VectorInsert, [vector, subvector]) => (0..*vector)
                .map(|lane| {
                    let inserted = usize::try_from(index)
                        .ok()
                        .and_then(|start| lane.checked_sub(start))
                        .filter(|position| position < subvector);
                    Some(inserted.map_or(lane, |position| vector + position))
                })
                .collect(),
            (Self::VectorSplice(direction), [width, _]) => {
                let lanes = i128::try_from(*width).ok()?;
                let start = (0..=lanes).contains(&index).then(|| match direction {
                    SpliceDirection::Left => index as usize,
                    SpliceDirection::Right => (lanes - index) as usize,
                });
                (0..*width)
                    .map(|lane| start.map(|start| start + lane))
                    .collect()
            }
            (Self::VectorInterleave2, [width, _]) => (0..2 * width)
                .map(|lane| Some(lane / 2 + if lane % 2 == 0 { 0 } else { *width }))
                .collect(),
            (Self::VectorDeinterleave2, [_]) => (0..2 * result_width)
                .map(|lane| Some(lane % result_width * 2 + lane / result_width))
                .collect(),
            _ => return None,
        };
        Some(sources)
    }

    /// Whether operand `position` is the `i1` flag `ctlz` and `cttz` take
    /// (whether a zero operand is poison), rather than a lane vector.
    pub const fn is_flag(self, position: usize) -> bool {
        matches!(self, Self::Ctlz | Self::Cttz) && position == 1
    }

    /// Why the intrinsic does not take `operands` and give `result`, as
    /// LLVM translation rejects the call, or `None` when it does. Every
    /// type must be closed. A lane vector is a `SIMD` type (a width-one
    /// one is the scalar); the elementwise intrinsics give their operand's
    /// type, the reductions one lane of a vector of at least two, the
    /// shuffles (`vector.extract`, `vector.insert`, the splices, and
    /// `vector.interleave2`) and `llvm.stepvector` a vector of at least two
    /// lanes, `vector.deinterleave2` a literal struct of two, and
    /// `llvm.donothing` `NoneType`.
    ///
    /// `result` is the type as LLVM translation sees it: a one-field
    /// register-passable struct is its field, as upstream's KGEN-to-LLVM
    /// lowering unwraps it, and the caller unwraps it before asking. A
    /// literal struct is a `TyKind::Tuple`, upstream's
    /// `!kgen.struct<…isParamPack>`, which is never unwrapped.
    pub fn signature_error(self, result: &Ty, operands: &[Ty]) -> Option<String> {
        if operands.len() != self.arity() {
            return Some(format!(
                "'{}' takes {} operand(s), got {}",
                self.name(),
                self.arity(),
                operands.len()
            ));
        }
        let lanes = |ty: &Ty| match simd_slots(ty.kind()) {
            Some((SimdDtype::Known(dtype), SimdWidth::Known(width))) => Some((dtype, width)),
            _ => None,
        };
        let mismatch = || {
            Some(format!(
                "the intrinsic signature of '{}' does not match: '{}' from ({})",
                self.name(),
                result,
                operands
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        match self {
            Self::Donothing => {
                return (!matches!(result.kind(), TyKind::None))
                    .then(mismatch)
                    .flatten();
            }
            Self::Stepvector => {
                return match lanes(result) {
                    Some((Dtype::Bool, width)) if width >= 2 => Some(
                        "stepvector only supported for vectors of integers with a bitwidth of \
                         at least 8."
                            .to_string(),
                    ),
                    Some((dtype, width)) if width >= 2 && dtype.is_integral() => None,
                    _ => mismatch(),
                };
            }
            Self::VectorDeinterleave2 if !matches!(result.kind(), TyKind::Tuple(elements) if elements.len() == 2) =>
            {
                return Some(
                    "intrinsic return type expected literal non-packed struct with 2 elements"
                        .to_string(),
                );
            }
            Self::VectorExtract
            | Self::VectorInsert
            | Self::VectorSplice(_)
            | Self::VectorInterleave2
            | Self::VectorDeinterleave2 => {
                return (!self.shuffle_signature(result, operands))
                    .then(mismatch)
                    .flatten();
            }
            _ => {}
        }
        let Some((dtype, width)) = operands.first().and_then(lanes) else {
            return mismatch();
        };
        let admitted = if self.takes_floats() {
            dtype.is_float()
        } else {
            dtype != Dtype::Bool && !dtype.is_float()
        };
        if !admitted {
            return Some(format!(
                "'{}' takes {} lanes, got '{}'",
                self.name(),
                if self.takes_floats() {
                    "float"
                } else {
                    "integer"
                },
                operands[0]
            ));
        }
        if let Self::VectorReduce(_) = self {
            return (width < 2 || lanes(result) != Some((dtype, 1)))
                .then(mismatch)
                .flatten();
        }
        let elementwise = operands.iter().enumerate().all(|(position, operand)| {
            if self.is_flag(position) {
                matches!(operand.kind(), TyKind::Bool) || lanes(operand) == Some((Dtype::Bool, 1))
            } else {
                operand == &operands[0]
            }
        });
        if !elementwise || result != &operands[0] || lanes(result) != Some((dtype, width)) {
            return mismatch();
        }
        if self == Self::Bswap && (dtype.bit_width() < 16 || dtype.bit_width() % 16 != 0) {
            return Some("bswap must be an even number of bytes".to_string());
        }
        None
    }

    /// Whether a shuffling intrinsic takes `operands` and gives `result`.
    /// Its lanes are any dtype's, every vector at least two lanes wide (a
    /// width-one `SIMD` lowers to the scalar), and its
    /// [`Self::index_operand`] a one-lane integer of the width LLVM
    /// declares: `vector.extract` gives a vector of its operand's lanes,
    /// `vector.insert` its first operand's type from a vector of its lanes,
    /// a splice its two operands' type, `vector.interleave2` twice its two
    /// operands' width, and `vector.deinterleave2` two vectors of half its
    /// operand's width.
    fn shuffle_signature(self, result: &Ty, operands: &[Ty]) -> bool {
        let vector = |ty: &Ty| match simd_slots(ty.kind()) {
            Some((SimdDtype::Known(dtype), SimdWidth::Known(width))) if width >= 2 => {
                Some((dtype, width))
            }
            _ => None,
        };
        let index = self.index_operand();
        if let Some((position, bits)) = index
            && !matches!(
                operands.get(position).map(|ty| simd_slots(ty.kind())),
                Some(Some((SimdDtype::Known(dtype), SimdWidth::Known(1))))
                    if dtype.is_integral() && dtype.bit_width() == bits
            )
        {
            return false;
        }
        let Some(vectors) = operands
            .iter()
            .enumerate()
            .filter(|(position, _)| index.map(|(index, _)| index) != Some(*position))
            .map(|(_, operand)| vector(operand))
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        if let TyKind::Tuple(halves) = result.kind() {
            return self == Self::VectorDeinterleave2
                && match (halves.as_slice(), vectors.as_slice()) {
                    ([even, odd], [(lanes, width)]) => {
                        even == odd
                            && vector(even).is_some_and(|half| half == (*lanes, width / 2))
                            && width % 2 == 0
                    }
                    _ => false,
                };
        }
        let Some((dtype, width)) = vector(result) else {
            return false;
        };
        match (self, vectors.as_slice()) {
            (Self::VectorExtract, [(lanes, _)]) => *lanes == dtype,
            (Self::VectorInsert, [vector, (lanes, _)]) => {
                *lanes == dtype && *vector == (dtype, width)
            }
            (Self::VectorSplice(_), [left, right]) => {
                *left == (dtype, width) && *right == (dtype, width)
            }
            (Self::VectorInterleave2, [left, right]) => {
                left == right && *left == (dtype, width / 2) && width % 2 == 0
            }
            _ => false,
        }
    }

    /// Why the comptime interpreter cannot call the intrinsic, or `None`
    /// when it can: it calls only what LLVM's constant folder folds, which
    /// leaves out `llvm.donothing`, the float reductions, `vector.insert`,
    /// the splices, and `llvm.stepvector`, and it types no struct result,
    /// which leaves out `vector.deinterleave2`.
    pub fn compile_time_error(self) -> Option<String> {
        if self.result_elements() > 1 {
            return Some("LLVM intrinsic has unknown result type".to_string());
        }
        match self {
            Self::Donothing => Some(format!(
                "cannot constant fold zero-result LLVM intrinsic: {}",
                self.name()
            )),
            Self::VectorReduce(VectorReduction::Fmax | VectorReduction::Fmin)
            | Self::VectorInsert
            | Self::VectorSplice(_)
            | Self::Stepvector => Some(format!(
                "LLVM could not constant fold intrinsic: {}",
                self.name()
            )),
            _ => None,
        }
    }

    /// Whether the intrinsic's lanes are floats rather than integers.
    pub const fn takes_floats(self) -> bool {
        matches!(
            self,
            Self::Copysign
                | Self::Sqrt
                | Self::VectorReduce(VectorReduction::Fmax | VectorReduction::Fmin)
        )
    }
}

/// One integer lane of the elementwise `intrinsic` over lanes of `dtype`.
///
/// Each lane is held sign- or zero-extended and computed on its bits at the
/// dtype's width; `None` is an intrinsic that is not elementwise over
/// integers. `ctlz` and `cttz` of zero give the width, whatever the flag (a
/// defined value refines LLVM's poison), and a funnel shift takes its
/// amount modulo the width.
pub fn int_lane(intrinsic: LlvmIntrinsic, dtype: Dtype, lanes: &[i128]) -> Option<i128> {
    let width = dtype.bit_width();
    let bits = |lane: i128| (lane as u128) & lane_mask(width);
    let lane = match (intrinsic, lanes) {
        (LlvmIntrinsic::Ctlz, [a, ..]) => {
            i128::from(bits(*a).leading_zeros() - (u128::BITS - width))
        }
        (LlvmIntrinsic::Cttz, [a, ..]) => i128::from(bits(*a).trailing_zeros().min(width)),
        (LlvmIntrinsic::Ctpop, [a]) => i128::from(bits(*a).count_ones()),
        (LlvmIntrinsic::Bitreverse, [a]) => {
            (bits(*a).reverse_bits() >> (u128::BITS - width)) as i128
        }
        (LlvmIntrinsic::Bswap, [a]) => (bits(*a).swap_bytes() >> (u128::BITS - width)) as i128,
        (LlvmIntrinsic::Fshl, [a, b, shift]) => {
            let shift = bits(*shift) % u128::from(width);
            let joined = (bits(*a) << width) | bits(*b);
            ((joined << shift) >> width) as i128
        }
        (LlvmIntrinsic::Fshr, [a, b, shift]) => {
            let shift = bits(*shift) % u128::from(width);
            let joined = (bits(*a) << width) | bits(*b);
            (joined >> shift) as i128
        }
        _ => return None,
    };
    Some(crate::ct::wrap_lane(dtype, lane))
}

/// One float lane of the elementwise `intrinsic` over lanes of `dtype`,
/// rounded once to the lane; `None` for an intrinsic that is not
/// elementwise over floats.
pub fn float_lane(intrinsic: LlvmIntrinsic, dtype: Dtype, lanes: &[f64]) -> Option<f64> {
    let lane = match (intrinsic, lanes) {
        (LlvmIntrinsic::Copysign, [magnitude, sign]) => magnitude.copysign(*sign),
        (LlvmIntrinsic::Sqrt, [a]) => a.sqrt(),
        _ => return None,
    };
    Some(dtype.round_lane(lane))
}

/// The lanes of `llvm.stepvector` over `width` lanes of `dtype`: each its
/// own position, wrapped to the lane.
pub fn step_lanes(dtype: Dtype, width: usize) -> Vec<i128> {
    (0..width)
        .map(|lane| crate::ct::wrap_lane(dtype, lane as i128))
        .collect()
}

/// The integer reduction of `lanes` of `dtype`, each held sign- or
/// zero-extended.
///
/// `smax` and `smin` read the lane bits signed and `umax` and `umin`
/// unsigned, whatever the dtype's own signedness; `None` is a float
/// reduction or no lanes.
pub fn reduce_ints(reduction: VectorReduction, dtype: Dtype, lanes: &[i128]) -> Option<i128> {
    let pick: fn(i128, i128) -> i128 = match reduction {
        VectorReduction::And => |a, b| a & b,
        VectorReduction::Or => |a, b| a | b,
        VectorReduction::Smax | VectorReduction::Umax => i128::max,
        VectorReduction::Smin | VectorReduction::Umin => i128::min,
        VectorReduction::Fmax | VectorReduction::Fmin => return None,
    };
    let width = dtype.bit_width();
    let signed = matches!(reduction, VectorReduction::Smax | VectorReduction::Smin);
    let read = |lane: &i128| {
        let unsigned = (*lane as u128 & lane_mask(width)) as i128;
        let shift = 128 - width;
        if signed {
            (unsigned << shift) >> shift
        } else {
            unsigned
        }
    };
    let lane = lanes.iter().map(read).reduce(pick)?;
    Some(crate::ct::wrap_lane(dtype, lane))
}

/// The float reduction of `lanes`: `fmax` and `fmin` are LLVM's `maxnum`
/// and `minnum`, which pass over a NaN lane; `None` for an integer
/// reduction or no lanes.
pub fn reduce_floats(reduction: VectorReduction, lanes: &[f64]) -> Option<f64> {
    let pick: fn(f64, f64) -> f64 = match reduction {
        VectorReduction::Fmax => f64::max,
        VectorReduction::Fmin => f64::min,
        _ => return None,
    };
    lanes.iter().copied().reduce(pick)
}

const fn lane_mask(width: u32) -> u128 {
    if width >= u128::BITS {
        u128::MAX
    } else {
        (1 << width) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_spellings_parse() {
        for intrinsic in LlvmIntrinsic::ALL {
            assert_eq!(LlvmIntrinsic::parse(intrinsic.name()), Some(intrinsic));
        }
        assert_eq!(LlvmIntrinsic::parse("llvm.no.such.thing"), None);
        assert_eq!(LlvmIntrinsic::parse("llvm.vector.reduce.add"), None);
    }

    #[test]
    fn lanes_follow_the_pin() {
        let ctlz = |dtype, a| int_lane(LlvmIntrinsic::Ctlz, dtype, &[a, 0]);
        assert_eq!(ctlz(Dtype::Int32, 1), Some(31));
        assert_eq!(ctlz(Dtype::Int64, 1), Some(63));
        assert_eq!(ctlz(Dtype::Int8, -1), Some(0));
        assert_eq!(ctlz(Dtype::Int32, 0), Some(32));
        assert_eq!(
            int_lane(LlvmIntrinsic::Cttz, Dtype::UInt8, &[8, 0]),
            Some(3)
        );
        assert_eq!(int_lane(LlvmIntrinsic::Ctpop, Dtype::Int8, &[-1]), Some(8));
        assert_eq!(
            int_lane(LlvmIntrinsic::Bitreverse, Dtype::Int8, &[1]),
            Some(-128)
        );
        assert_eq!(
            int_lane(LlvmIntrinsic::Bswap, Dtype::UInt16, &[0x1234]),
            Some(0x3412)
        );
        assert_eq!(
            int_lane(LlvmIntrinsic::Fshl, Dtype::UInt8, &[0x81, 0x81, 9]),
            Some(3)
        );
        assert_eq!(
            int_lane(LlvmIntrinsic::Fshl, Dtype::Int, &[1, 2, 3]),
            Some(8)
        );
        assert_eq!(
            int_lane(LlvmIntrinsic::Fshr, Dtype::UInt8, &[0x01, 0x80, 1]),
            Some(0xC0)
        );
        assert_eq!(
            reduce_ints(VectorReduction::Smax, Dtype::UInt8, &[1, 200, 3, 4]),
            Some(4)
        );
        assert_eq!(
            reduce_ints(VectorReduction::Umax, Dtype::UInt8, &[1, 200, 3, 4]),
            Some(200)
        );
        assert_eq!(
            reduce_ints(VectorReduction::Umin, Dtype::Int8, &[-1, 3]),
            Some(3)
        );
        assert_eq!(
            reduce_floats(VectorReduction::Fmax, &[f64::NAN, 2.0]),
            Some(2.0)
        );
        assert_eq!(
            float_lane(LlvmIntrinsic::Copysign, Dtype::Float64, &[2.0, -0.0]),
            Some(-2.0)
        );
    }

    #[test]
    fn signatures_follow_llvm() {
        let int8 = Ty::from(TyKind::Simd {
            dtype: SimdDtype::Known(Dtype::Int8),
            width: SimdWidth::Known(1),
        });
        assert_eq!(
            LlvmIntrinsic::Bswap.signature_error(&int8, std::slice::from_ref(&int8)),
            Some("bswap must be an even number of bytes".to_string())
        );
        assert_eq!(
            LlvmIntrinsic::Ctpop.signature_error(&int8, std::slice::from_ref(&int8)),
            None
        );
        assert!(
            LlvmIntrinsic::Ctlz
                .signature_error(&int8, std::slice::from_ref(&int8))
                .is_some()
        );
        assert_eq!(
            LlvmIntrinsic::Ctlz.signature_error(&int8, &[int8.clone(), Ty::bool()]),
            None
        );
        assert!(
            LlvmIntrinsic::VectorReduce(VectorReduction::Smax)
                .signature_error(&int8, std::slice::from_ref(&int8))
                .is_some()
        );
        assert_eq!(
            LlvmIntrinsic::Donothing.signature_error(&Ty::none(), &[]),
            None
        );
    }

    #[test]
    fn shuffles_follow_the_pin() {
        let sources = |intrinsic: LlvmIntrinsic, result, widths: &[usize], index| {
            intrinsic
                .shuffle_sources(result, widths, index)
                .map(|sources| sources.into_iter().collect::<Option<Vec<_>>>())
        };
        assert_eq!(
            sources(LlvmIntrinsic::VectorExtract, 4, &[8], 4),
            Some(Some(vec![4, 5, 6, 7]))
        );
        assert_eq!(
            sources(LlvmIntrinsic::VectorInsert, 8, &[8, 2], 2),
            Some(Some(vec![0, 1, 8, 9, 4, 5, 6, 7]))
        );
        let left = LlvmIntrinsic::VectorSplice(SpliceDirection::Left);
        let right = LlvmIntrinsic::VectorSplice(SpliceDirection::Right);
        assert_eq!(sources(left, 4, &[4, 4], 1), Some(Some(vec![1, 2, 3, 4])));
        assert_eq!(sources(right, 4, &[4, 4], 1), Some(Some(vec![3, 4, 5, 6])));
        assert_eq!(sources(right, 4, &[4, 4], 0), Some(Some(vec![4, 5, 6, 7])));
        assert_eq!(sources(left, 4, &[4, 4], 5), Some(None));
        assert_eq!(
            sources(LlvmIntrinsic::VectorInterleave2, 8, &[4, 4], 0),
            Some(Some(vec![0, 4, 1, 5, 2, 6, 3, 7]))
        );
        assert_eq!(
            sources(LlvmIntrinsic::VectorDeinterleave2, 4, &[8], 0),
            Some(Some(vec![0, 2, 4, 6, 1, 3, 5, 7]))
        );
        assert_eq!(sources(LlvmIntrinsic::Ctpop, 4, &[4], 0), None);
        assert_eq!(step_lanes(Dtype::UInt8, 3), vec![0, 1, 2]);
        assert!(
            LlvmIntrinsic::VectorExtract
                .index_error(4, &[8], 2)
                .is_some_and(|reason| reason.contains("constant multiple"))
        );
        assert_eq!(
            LlvmIntrinsic::VectorExtract.index_error(4, &[8], 8),
            Some("vector_extract would overrun.".to_string())
        );
        assert_eq!(LlvmIntrinsic::VectorInsert.index_error(8, &[8, 2], 6), None);
        assert_eq!(left.index_error(4, &[4, 4], 9), None);
    }

    #[test]
    fn shuffle_signatures_follow_llvm() {
        let simd = |dtype, width| {
            Ty::from(TyKind::Simd {
                dtype: SimdDtype::Known(dtype),
                width: SimdWidth::Known(width),
            })
        };
        let (quad, octet) = (simd(Dtype::Int32, 4), simd(Dtype::Int32, 8));
        let (index, offset) = (simd(Dtype::Int64, 1), simd(Dtype::Int32, 1));
        assert_eq!(
            LlvmIntrinsic::VectorExtract.signature_error(&quad, &[octet.clone(), index.clone()]),
            None
        );
        assert!(
            LlvmIntrinsic::VectorExtract
                .signature_error(&quad, &[octet.clone(), offset.clone()])
                .is_some()
        );
        assert!(
            LlvmIntrinsic::VectorExtract
                .signature_error(&simd(Dtype::Int32, 1), &[octet.clone(), index.clone()])
                .is_some()
        );
        assert_eq!(
            LlvmIntrinsic::VectorInsert
                .signature_error(&octet, &[octet.clone(), quad.clone(), index]),
            None
        );
        let left = LlvmIntrinsic::VectorSplice(SpliceDirection::Left);
        assert_eq!(
            left.signature_error(&quad, &[quad.clone(), quad.clone(), offset]),
            None
        );
        assert_eq!(
            LlvmIntrinsic::VectorInterleave2.signature_error(&octet, &[quad.clone(), quad.clone()]),
            None
        );
        assert!(
            LlvmIntrinsic::VectorInterleave2
                .signature_error(&quad, &[quad.clone(), quad.clone()])
                .is_some()
        );
        let deinterleave = LlvmIntrinsic::VectorDeinterleave2;
        let halves = |half: &Ty| Ty::from(TyKind::Tuple(vec![half.clone(), half.clone()]));
        assert_eq!(
            deinterleave.signature_error(&halves(&quad), std::slice::from_ref(&octet)),
            None
        );
        assert!(
            deinterleave
                .signature_error(&quad, std::slice::from_ref(&octet))
                .is_some_and(|reason| reason.contains("expected literal non-packed struct"))
        );
        assert!(
            deinterleave
                .signature_error(&halves(&octet), std::slice::from_ref(&octet))
                .is_some()
        );
        assert!(
            deinterleave
                .signature_error(&halves(&simd(Dtype::Int32, 1)), &[simd(Dtype::Int32, 2)])
                .is_some()
        );
        assert!(
            LlvmIntrinsic::VectorInterleave2
                .signature_error(&halves(&quad), &[quad.clone(), quad.clone()])
                .is_some()
        );
        assert_eq!(LlvmIntrinsic::Stepvector.signature_error(&quad, &[]), None);
        assert!(
            LlvmIntrinsic::Stepvector
                .signature_error(&simd(Dtype::Bool, 4), &[])
                .is_some_and(|reason| reason.starts_with("stepvector only supported"))
        );
        assert!(
            LlvmIntrinsic::Stepvector
                .signature_error(&simd(Dtype::Float32, 4), &[])
                .is_some()
        );
    }

    #[test]
    fn compile_time_calls_follow_the_pin() {
        assert_eq!(LlvmIntrinsic::Ctpop.compile_time_error(), None);
        assert_eq!(LlvmIntrinsic::VectorExtract.compile_time_error(), None);
        assert_eq!(LlvmIntrinsic::VectorInterleave2.compile_time_error(), None);
        for intrinsic in [
            LlvmIntrinsic::Donothing,
            LlvmIntrinsic::VectorReduce(VectorReduction::Fmax),
            LlvmIntrinsic::VectorInsert,
            LlvmIntrinsic::VectorSplice(SpliceDirection::Right),
            LlvmIntrinsic::Stepvector,
            LlvmIntrinsic::VectorDeinterleave2,
        ] {
            assert!(intrinsic.compile_time_error().is_some());
        }
    }
}
