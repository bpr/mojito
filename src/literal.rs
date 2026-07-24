//! Exact compile-time numeric literal values.
//!
//! Mojo's `IntLiteral` and `FloatLiteral` are not machine integers/floats: they
//! retain arbitrary precision until a checked context materializes them as a
//! runtime scalar. Integers use [`BigInt`]; finite floating literals use an exact
//! rational so calculations such as `3.0 * (4.0 / 3.0 - 1.0)` do not round at
//! each intermediate operation. A separate negative-zero bit preserves the one
//! finite IEEE distinction a rational alone cannot represent.

use std::fmt;

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

/// Prevent a short source spelling such as `1e999999999999` or `2 ** huge`
/// from asking the compiler to allocate an unbounded amount of memory.  This is
/// a compiler resource limit, not a numeric precision limit: values with up to
/// one million decimal/binary exponent steps remain exact.
pub const MAX_LITERAL_EXPONENT: u64 = 1_000_000;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntLiteral(BigInt);

impl IntLiteral {
    pub fn parse_radix(digits: &str, radix: u32) -> Option<Self> {
        BigInt::parse_bytes(digits.as_bytes(), radix).map(Self)
    }

    pub fn as_bigint(&self) -> &BigInt {
        &self.0
    }

    pub fn into_bigint(self) -> BigInt {
        self.0
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    pub fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }

    pub fn to_i128(&self) -> Option<i128> {
        self.0.to_i128()
    }

    pub fn to_f64(&self) -> Option<f64> {
        self.0.to_f64()
    }

    /// Materialize into an unsigned two's-complement lane of `bits` bits.
    /// Mojo scalar materialization is wrapping; the literal calculation that
    /// precedes it remains unbounded.
    pub fn wrapping_unsigned(&self, bits: u32) -> Option<u64> {
        if bits == 0 || bits > 64 {
            return None;
        }
        let modulus = BigInt::one() << bits;
        self.0.mod_floor(&modulus).to_u64()
    }

    /// Materialize into a signed two's-complement lane of `bits` bits.
    pub fn wrapping_signed(&self, bits: u32) -> Option<i64> {
        if bits == 0 || bits > 64 {
            return None;
        }
        let modulus = BigInt::one() << bits;
        let sign_bit = BigInt::one() << (bits - 1);
        let unsigned = self.0.mod_floor(&modulus);
        let signed = if unsigned >= sign_bit {
            unsigned - modulus
        } else {
            unsigned
        };
        signed.to_i64()
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    pub fn is_negative(&self) -> bool {
        self.0.sign() == Sign::Minus
    }

    pub fn neg(&self) -> Self {
        Self(-&self.0)
    }

    pub fn add(&self, rhs: &Self) -> Self {
        Self(&self.0 + &rhs.0)
    }

    pub fn sub(&self, rhs: &Self) -> Self {
        Self(&self.0 - &rhs.0)
    }

    pub fn mul(&self, rhs: &Self) -> Self {
        Self(&self.0 * &rhs.0)
    }

    pub fn floor_div(&self, rhs: &Self) -> Option<Self> {
        (!rhs.is_zero()).then(|| Self(self.0.div_floor(&rhs.0)))
    }

    pub fn floor_mod(&self, rhs: &Self) -> Option<Self> {
        (!rhs.is_zero()).then(|| Self(self.0.mod_floor(&rhs.0)))
    }

    pub fn pow(&self, exponent: &Self) -> Option<Self> {
        let exponent = exponent.to_u64()?;
        if exponent > MAX_LITERAL_EXPONENT {
            return None;
        }
        let exponent = u32::try_from(exponent).ok()?;
        Some(Self(self.0.pow(exponent)))
    }

    pub fn shl(&self, rhs: &Self) -> Option<Self> {
        let shift = rhs.to_u64()?;
        if shift > MAX_LITERAL_EXPONENT {
            return None;
        }
        Some(Self(&self.0 << usize::try_from(shift).ok()?))
    }

    pub fn shr(&self, rhs: &Self) -> Option<Self> {
        let shift = rhs.to_u64()?;
        if shift > MAX_LITERAL_EXPONENT {
            return None;
        }
        Some(Self(&self.0 >> usize::try_from(shift).ok()?))
    }

    pub fn bitand(&self, rhs: &Self) -> Self {
        Self(&self.0 & &rhs.0)
    }

    pub fn bitor(&self, rhs: &Self) -> Self {
        Self(&self.0 | &rhs.0)
    }

    pub fn bitxor(&self, rhs: &Self) -> Self {
        Self(&self.0 ^ &rhs.0)
    }
}

impl From<i64> for IntLiteral {
    fn from(value: i64) -> Self {
        Self(value.into())
    }
}

impl From<i32> for IntLiteral {
    fn from(value: i32) -> Self {
        Self(value.into())
    }
}

impl From<u32> for IntLiteral {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}

impl From<u64> for IntLiteral {
    fn from(value: u64) -> Self {
        Self(value.into())
    }
}

impl From<usize> for IntLiteral {
    fn from(value: usize) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for IntLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for IntLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FloatLiteral {
    value: BigRational,
    negative_zero: bool,
}

impl FloatLiteral {
    /// Parse a finite decimal spelling after `_` separators have been removed.
    pub fn parse_decimal(text: &str) -> Option<Self> {
        let (mantissa, exponent) = match text.find(['e', 'E']) {
            Some(index) => (&text[..index], text[index + 1..].parse::<i64>().ok()?),
            None => (text, 0),
        };
        let (whole, fraction) = match mantissa.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (mantissa, ""),
        };
        let whole = if whole.is_empty() { "0" } else { whole };
        let digits = format!("{whole}{fraction}");
        let mut numerator = BigInt::parse_bytes(digits.as_bytes(), 10)?;
        let scale = i64::try_from(fraction.len()).ok()?.checked_sub(exponent)?;
        let value = if scale >= 0 {
            let power = u32::try_from(scale).ok()?;
            if u64::from(power) > MAX_LITERAL_EXPONENT {
                return None;
            }
            BigRational::new(numerator, BigInt::from(10u8).pow(power))
        } else {
            let power = u32::try_from(scale.checked_neg()?).ok()?;
            if u64::from(power) > MAX_LITERAL_EXPONENT {
                return None;
            }
            numerator *= BigInt::from(10u8).pow(power);
            BigRational::from_integer(numerator)
        };
        Some(Self {
            value,
            negative_zero: false,
        })
    }

    /// Exact representation of an already-materialized binary float. This is
    /// used only for compiler-synthesized constants and compatibility helpers;
    /// source literals always enter through [`Self::parse_decimal`].
    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        Some(Self {
            value: BigRational::from_float(value)?,
            negative_zero: value == 0.0 && value.is_sign_negative(),
        })
    }

    pub fn from_int(value: &IntLiteral) -> Self {
        Self {
            value: BigRational::from_integer(value.as_bigint().clone()),
            negative_zero: false,
        }
    }

    pub fn as_rational(&self) -> &BigRational {
        &self.value
    }

    pub fn is_zero(&self) -> bool {
        self.value.is_zero()
    }

    pub fn is_negative_zero(&self) -> bool {
        self.negative_zero
    }

    pub fn is_negative(&self) -> bool {
        self.sign_negative()
    }

    pub fn abs(&self) -> Self {
        Self {
            value: self.value.abs(),
            negative_zero: false,
        }
    }

    pub fn neg(&self) -> Self {
        Self {
            value: -&self.value,
            negative_zero: self.is_zero() && !self.negative_zero,
        }
    }

    pub fn add(&self, rhs: &Self) -> Self {
        let mut result = Self::from_rational(&self.value + &rhs.value);
        // IEEE addition preserves -0 only when both zero operands are -0.
        // Exact cancellation of nonzero values, and mixed-sign zero addition,
        // produce +0 under round-to-nearest.
        if result.is_zero() {
            result.negative_zero =
                self.is_zero() && rhs.is_zero() && self.negative_zero && rhs.negative_zero;
        }
        result
    }

    pub fn sub(&self, rhs: &Self) -> Self {
        self.add(&rhs.neg())
    }

    pub fn mul(&self, rhs: &Self) -> Self {
        let mut result = Self::from_rational(&self.value * &rhs.value);
        if result.is_zero() {
            result.negative_zero = self.sign_negative() ^ rhs.sign_negative();
        }
        result
    }

    pub fn div(&self, rhs: &Self) -> Option<Self> {
        if rhs.is_zero() {
            return None;
        }
        let mut result = Self::from_rational(&self.value / &rhs.value);
        if result.is_zero() {
            result.negative_zero = self.sign_negative() ^ rhs.sign_negative();
        }
        Some(result)
    }

    pub fn floor_div(&self, rhs: &Self) -> Option<Self> {
        let quotient = self.div(rhs)?;
        Some(Self::from_rational(BigRational::from_integer(
            quotient.value.numer().div_floor(quotient.value.denom()),
        )))
    }

    pub fn floor_mod(&self, rhs: &Self) -> Option<Self> {
        let quotient = self.floor_div(rhs)?;
        Some(self.sub(&rhs.mul(&quotient)))
    }

    pub fn pow_int(&self, exponent: &IntLiteral) -> Option<Self> {
        let exponent = exponent.to_i64()?;
        let magnitude = u32::try_from(exponent.unsigned_abs()).ok()?;
        if u64::from(magnitude) > MAX_LITERAL_EXPONENT {
            return None;
        }
        let value = if exponent >= 0 {
            self.value.pow(magnitude as i32)
        } else {
            if self.is_zero() {
                return None;
            }
            self.value.pow(-(magnitude as i32))
        };
        Some(Self::from_rational(value))
    }

    pub fn trunc_to_int(&self) -> IntLiteral {
        IntLiteral(self.value.to_integer())
    }

    pub fn to_int_if_whole(&self) -> Option<IntLiteral> {
        self.value.is_integer().then(|| self.trunc_to_int())
    }

    /// Numeric comparison deliberately ignores the stored sign of zero.  The
    /// sign bit is representation state needed for later IEEE materialization,
    /// while Mojo numeric equality follows IEEE's `-0.0 == 0.0` rule.
    pub fn numeric_cmp(&self, rhs: &Self) -> std::cmp::Ordering {
        self.value.cmp(&rhs.value)
    }

    pub fn to_f64(&self) -> Option<f64> {
        if self.is_zero() && self.negative_zero {
            Some(-0.0)
        } else {
            self.value.to_f64()
        }
    }

    /// Correctly round an exact literal directly to IEEE binary32.  Going via
    /// binary64 can double-round values immediately beside an f32 midpoint, so
    /// choose between the cast's neighboring f32 values using exact rational
    /// distances.
    pub fn to_f32(&self) -> Option<f32> {
        if self.is_zero() {
            return Some(if self.negative_zero { -0.0 } else { 0.0 });
        }

        let negative = self.value.is_negative();
        let magnitude = self.value.abs();
        let overflow_threshold =
            BigRational::from_integer((BigInt::one() << 128usize) - (BigInt::one() << 103usize));
        if magnitude >= overflow_threshold {
            return Some(if negative {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            });
        }

        let approximation = magnitude.to_f64()? as f32;
        let mut candidates = Vec::with_capacity(3);
        if approximation.is_finite() {
            candidates.push(approximation);
            candidates.push(f32_next_down(approximation));
            let next = f32_next_up(approximation);
            if next.is_finite() {
                candidates.push(next);
            }
        } else {
            candidates.push(f32::MAX);
        }
        candidates.sort_by_key(|value| value.to_bits());
        candidates.dedup_by_key(|value| value.to_bits());

        let best = candidates.into_iter().min_by(|left, right| {
            let left_value = BigRational::from_float(*left)
                .expect("finite f32 has an exact rational representation");
            let right_value = BigRational::from_float(*right)
                .expect("finite f32 has an exact rational representation");
            let left_distance = (&magnitude - left_value).abs();
            let right_distance = (&magnitude - right_value).abs();
            left_distance.cmp(&right_distance).then_with(|| {
                // IEEE roundTiesToEven: bit zero is the low significand bit for
                // both normal and subnormal finite f32 encodings.
                (left.to_bits() & 1).cmp(&(right.to_bits() & 1))
            })
        })?;
        Some(if negative { -best } else { best })
    }

    fn from_rational(value: BigRational) -> Self {
        Self {
            value,
            negative_zero: false,
        }
    }

    fn sign_negative(&self) -> bool {
        self.value.is_negative() || (self.value.is_zero() && self.negative_zero)
    }
}

impl From<f64> for FloatLiteral {
    fn from(value: f64) -> Self {
        Self::from_f64(value).expect("FloatLiteral::from only accepts finite f64 values")
    }
}

impl From<f32> for FloatLiteral {
    fn from(value: f32) -> Self {
        Self::from_f64(f64::from(value)).expect("FloatLiteral::from only accepts finite f32 values")
    }
}

fn f32_next_up(value: f32) -> f32 {
    if value.is_nan() || value == f32::INFINITY {
        return value;
    }
    if value == -0.0 {
        return f32::from_bits(1);
    }
    let bits = value.to_bits();
    f32::from_bits(if value >= 0.0 { bits + 1 } else { bits - 1 })
}

fn f32_next_down(value: f32) -> f32 {
    if value.is_nan() || value == f32::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return f32::from_bits(0x8000_0001);
    }
    let bits = value.to_bits();
    f32::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
}

impl fmt::Debug for FloatLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for FloatLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() && self.negative_zero {
            return f.write_str("-0.0");
        }
        if self.value.denom().is_one() {
            write!(f, "{}.0", self.value.numer())
        } else {
            write!(f, "{}/{}", self.value.numer(), self.value.denom())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_materialization_wraps_after_exact_arithmetic() {
        let huge = IntLiteral::from(2)
            .pow(&IntLiteral::from(200))
            .expect("exact power");
        assert!(huge.to_i64().is_none());
        assert_eq!(huge.wrapping_signed(64), Some(0));
        assert_eq!(IntLiteral::from(200).wrapping_signed(8), Some(-56));
        assert_eq!(IntLiteral::from(-1).wrapping_unsigned(8), Some(255));
    }

    #[test]
    fn decimal_float_rounding_is_direct_and_ties_to_even() {
        let halfway = FloatLiteral::parse_decimal("1.000000059604644775390625").unwrap();
        let just_above =
            FloatLiteral::parse_decimal("1.0000000596046447753906250000000001").unwrap();
        assert_eq!(halfway.to_f32().unwrap().to_bits(), 1.0f32.to_bits());
        assert_eq!(just_above.to_f32().unwrap().to_bits(), 1.0f32.to_bits() + 1);

        let f64_halfway = FloatLiteral::parse_decimal("9007199254740993.0").unwrap();
        assert_eq!(f64_halfway.to_f64(), Some(9_007_199_254_740_992.0));
    }

    #[test]
    fn negative_zero_and_resource_quota_are_preserved() {
        let negative_zero = FloatLiteral::parse_decimal("0.0").unwrap().neg();
        let positive_zero = FloatLiteral::parse_decimal("0.0").unwrap();
        assert_eq!(
            negative_zero.to_f64().unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            negative_zero.numeric_cmp(&positive_zero),
            std::cmp::Ordering::Equal
        );
        assert!(negative_zero.add(&negative_zero).is_negative_zero());
        assert!(!negative_zero.add(&positive_zero).is_negative_zero());
        assert!(FloatLiteral::parse_decimal("1e1000001").is_none());
        assert!(
            IntLiteral::from(2)
                .pow(&IntLiteral::from(MAX_LITERAL_EXPONENT + 1))
                .is_none()
        );
    }
}
