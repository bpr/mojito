//! Narrow float lanes held as doubles.
//!
//! The VM, compile-time evaluation, and the native lowering compute `Float16`
//! and `Float32` arithmetic at double precision and round each result to the
//! lane once; these are the half-precision halves of that rounding, and the
//! exact codecs between a narrow lane's bits and the double holding it.
//!
//! A narrow lane's double holds its exact bits: a NaN keeps its sign, quiet
//! bit, and payload in the top of the double's mantissa, so a lane moved or
//! reinterpreted without arithmetic reads back the bits it was given, as a
//! register does at the pin. Arithmetic on a held signaling NaN quiets it as
//! the narrow instruction would.

pub use half::f16;

/// Round a double to the nearest binary16 value, ties to even, and widen it
/// back exactly.
pub fn round_f16(value: f64) -> f64 {
    f16::from_f64(value).to_f64()
}

/// The binary16 encoding of a double rounded to half precision.
pub fn f16_bits(value: f64) -> u16 {
    f16::from_f64(value).to_bits()
}

/// The double a binary16 encoding denotes, widened exactly.
pub fn f16_from_bits(bits: u16) -> f64 {
    f16::from_bits(bits).to_f64()
}

/// The double holding the binary32 lane `bits` exactly.
pub fn f32_lane_from_bits(bits: u32) -> f64 {
    let value = f32::from_bits(bits);
    if !value.is_nan() {
        return f64::from(value);
    }
    f64::from_bits(nan_bits(
        u64::from(bits >> 31),
        u64::from(bits & 0x007F_FFFF) << F32_MANTISSA_SHIFT,
    ))
}

/// The binary32 bits of the lane `value` holds: a NaN's sign and mantissa
/// truncated back ([`f32_lane_from_bits`]'s inverse), any other value
/// rounded to single precision.
pub const fn f32_lane_bits(value: f64) -> u32 {
    if !value.is_nan() {
        return (value as f32).to_bits();
    }
    let (sign, mantissa) = nan_parts(value, F32_MANTISSA_SHIFT);
    (sign as u32) << 31 | 0x7F80_0000 | mantissa as u32
}

/// The double holding the binary16 lane `bits` exactly.
pub fn f16_lane_from_bits(bits: u16) -> f64 {
    if !f16::from_bits(bits).is_nan() {
        return f16_from_bits(bits);
    }
    f64::from_bits(nan_bits(
        u64::from(bits >> 15),
        u64::from(bits & 0x03FF) << F16_MANTISSA_SHIFT,
    ))
}

/// The binary16 bits of the lane `value` holds ([`f16_lane_from_bits`]'s
/// inverse), any value but a NaN rounded to half precision.
pub fn f16_lane_bits(value: f64) -> u16 {
    if !value.is_nan() {
        return f16_bits(value);
    }
    let (sign, mantissa) = nan_parts(value, F16_MANTISSA_SHIFT);
    (sign as u16) << 15 | 0x7C00 | mantissa as u16
}

/// `value` with its NaN quieted, as a conversion between float widths
/// (`fpext`, `fptrunc`) quiets a signaling NaN; any other value unchanged.
pub const fn quiet_nan(value: f64) -> f64 {
    if value.is_nan() {
        f64::from_bits(value.to_bits() | QUIET_BIT)
    } else {
        value
    }
}

/// How far a binary32 mantissa sits below the top of a double's.
const F32_MANTISSA_SHIFT: u32 = 52 - 23;

/// How far a binary16 mantissa sits below the top of a double's.
const F16_MANTISSA_SHIFT: u32 = 52 - 10;

/// A double's quiet-NaN bit, the top of its mantissa.
const QUIET_BIT: u64 = 1 << 51;

/// The double NaN of `sign` whose mantissa is `mantissa`.
const fn nan_bits(sign: u64, mantissa: u64) -> u64 {
    sign << 63 | 0x7FF0_0000_0000_0000 | mantissa
}

/// The sign of the NaN `value` and its mantissa truncated to a narrow
/// lane's `52 - shift` bits. A payload the truncation empties keeps the
/// quiet bit, so the narrow lane stays a NaN.
const fn nan_parts(value: f64, shift: u32) -> (u64, u64) {
    let bits = value.to_bits();
    let mantissa = match (bits & 0x000F_FFFF_FFFF_FFFF) >> shift {
        0 => QUIET_BIT >> shift,
        mantissa => mantissa,
    };
    (bits >> 63, mantissa)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_precision_lane_rounding_ties_to_even() {
        // Each rounding lands on an exact binary16 neighbour, so the encodings
        // are what the test means: comparing them states that exactness where
        // float equality would only approximate it.
        let rounded = |value: f64| round_f16(value).to_bits();
        assert_eq!(rounded(0.800_048_828_125), 0.799_804_687_5f64.to_bits());
        assert_eq!(
            rounded(0.300_048_828_125 + 0.5),
            0.799_804_687_5f64.to_bits()
        );
        assert_eq!(rounded(65519.99), 65504.0f64.to_bits());
        assert_eq!(rounded(65520.0), f64::INFINITY.to_bits());
        assert_eq!(rounded(-70000.0), f64::NEG_INFINITY.to_bits());
        assert_eq!(rounded(1e-6), (17.0 * 2f64.powi(-24)).to_bits());
        assert!(round_f16(-0.0).is_sign_negative());
        assert!(round_f16(f64::NAN).is_nan());
        assert_eq!(f16_bits(0.8), 0x3a66);
    }

    #[test]
    fn a_narrow_lane_keeps_its_exact_bits() {
        for bits in [
            0x7F80_0001,
            0x7FC0_0000,
            0x7FC1_2345,
            0xFF80_0003,
            0x7F80_0000,
            0x8000_0000,
            0,
            1,
            0x3F80_0000,
        ] {
            assert_eq!(f32_lane_bits(f32_lane_from_bits(bits)), bits, "{bits:#x}");
        }
        for bits in [0x7C01, 0x7E00, 0x7E15, 0xFC02, 0x7C00, 0x8000, 0, 1, 0x3C00] {
            assert_eq!(f16_lane_bits(f16_lane_from_bits(bits)), bits, "{bits:#x}");
        }
        assert_eq!(f32_lane_from_bits(0x3F80_0000), 1.0);
        assert_eq!(
            f32_lane_bits(f64::from_bits(0x7FF0_0000_0000_0001)),
            0x7FC0_0000
        );
    }

    #[test]
    fn a_width_conversion_quiets_a_signaling_nan() {
        let signaling = f32_lane_from_bits(0x7F80_0001);
        assert_eq!(f32_lane_bits(quiet_nan(signaling)), 0x7FC0_0001);
        assert_eq!(quiet_nan(1.5), 1.5);
    }
}
