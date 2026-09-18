//! Half-precision float lanes.
//!
//! The VM, compile-time evaluation, and the native lowering compute `Float16`
//! and `Float32` arithmetic at double precision and round each result to the
//! lane once; these are the half-precision halves of that rounding.

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
}
