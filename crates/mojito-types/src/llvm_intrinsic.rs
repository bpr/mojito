//! The LLVM intrinsics `pop.call_llvm_intrinsic` reaches in Mojito.
//!
//! Upstream's `std.sys.intrinsics.llvm_intrinsic[name, T]` calls any LLVM
//! intrinsic by name, and LLVM translation rejects a name it does not know
//! or a signature the intrinsic does not take. Mojito computes a table of
//! them: the checker rejects a closed name outside it, the elaborator a
//! reached instance's, and the concrete verifier a signature
//! [`LlvmIntrinsic::signature_error`] refuses. The VM computes a lane by
//! [`int_lane`], [`float_lane`], [`reduce_ints`], and [`reduce_floats`], and
//! the native backend calls the intrinsic, mangled by
//! [`LlvmIntrinsic::overloads`].

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
    pub const ALL: [Self; 18] = [
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
            Self::Donothing => 0,
            Self::Ctpop | Self::Bitreverse | Self::Bswap | Self::Sqrt | Self::VectorReduce(_) => 1,
            Self::Ctlz | Self::Cttz | Self::Copysign => 2,
            Self::Fshl | Self::Fshr => 3,
        }
    }

    /// The types the intrinsic's LLVM name is mangled by, in order.
    pub const fn overloads(self) -> &'static [OverloadSlot] {
        match self {
            Self::Donothing => &[],
            Self::VectorReduce(_) => &[OverloadSlot::Operand(0)],
            _ => &[OverloadSlot::Result],
        }
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
    /// type, the reductions one lane of a vector of at least two, and
    /// `llvm.donothing` `NoneType`.
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
        if self == Self::Donothing {
            return (!matches!(result.kind(), TyKind::None))
                .then(mismatch)
                .flatten();
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
}
