//! The x86 arm of LLVM's `sys::getHostCPUName`, ported from
//! `llvm/lib/TargetParser/Host.cpp` at `llvmorg-23.1.0`: the pin's
//! `--target-cpu` defaults to the name this routine returns.
//!
//! The port follows the upstream functions one for one —
//! `getVendorSignature`, `detectX86FamilyModel`, `getAvailableFeatures`, and
//! the Intel, AMD, and Hygon `get…ProcessorTypeAndSubtype` arms — over an
//! injected [`Cpuid`] source, so recorded leaves can drive it. It drops the
//! `Type`/`Subtype` outputs, which upstream keeps only for parity with
//! compiler-rt. Re-diff it against `Host.cpp` whenever the pin's LLVM moves.

use std::arch::x86_64::{__cpuid_count, _xgetbv};

/// The host CPU's LLVM name, or `"generic"` when the vendor, family, or
/// model is unknown.
pub(super) fn host_cpu_name() -> &'static str {
    cpu_name(&HostCpuid)
}

/// A source of `cpuid` leaves and the `XCR0` register.
trait Cpuid {
    /// The raw `[eax, ebx, ecx, edx]` of `cpuid` at `leaf`/`subleaf`, with
    /// no range check.
    fn cpuid(&self, leaf: u32, subleaf: u32) -> [u32; 4];

    /// `XGETBV(0)`. Called only when `CPUID.1:ECX` reports both OSXSAVE
    /// (bit 27) and AVX (bit 28), as `getAvailableFeatures` guards it.
    fn xcr0(&self) -> u64;
}

/// The executing processor.
struct HostCpuid;

impl Cpuid for HostCpuid {
    fn cpuid(&self, leaf: u32, subleaf: u32) -> [u32; 4] {
        let result = __cpuid_count(leaf, subleaf);
        [result.eax, result.ebx, result.ecx, result.edx]
    }

    fn xcr0(&self) -> u64 {
        // SAFETY: `Cpuid::xcr0` runs only behind `getAvailableFeatures`'s
        // guard that `CPUID.1:ECX` sets OSXSAVE (bit 27), so the OS has
        // enabled `XGETBV` and the `xsave` target feature is present.
        unsafe { _xgetbv(0) }
    }
}

/// `sys::getHostCPUName`'s x86 body.
fn cpu_name(cpuid: &impl Cpuid) -> &'static str {
    let Some((vendor, max_leaf)) = vendor_signature(cpuid) else {
        return "generic";
    };
    let [eax, _, ecx, edx] = get_cpuid(cpuid, 1, 0).unwrap_or_default();
    let (family, model) = detect_family_model(eax);
    let features = available_features(cpuid, ecx, edx, max_leaf);
    match vendor {
        Vendor::GenuineIntel => intel_cpu(family, model, features),
        Vendor::AuthenticAmd => amd_cpu(family, model, features),
        Vendor::HygonGenuine => hygon_cpu(family, model),
    }
    .unwrap_or("generic")
}

/// GCC's `__get_cpuid_count`: `None` when `leaf` exceeds its range's
/// maximum (read from leaf `leaf & 0x8000_0000`) or that maximum is 0.
fn get_cpuid(cpuid: &impl Cpuid, leaf: u32, subleaf: u32) -> Option<[u32; 4]> {
    let max = cpuid.cpuid(leaf & 0x8000_0000, 0)[0];
    (max != 0 && max >= leaf).then(|| cpuid.cpuid(leaf, subleaf))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vendor {
    GenuineIntel,
    AuthenticAmd,
    HygonGenuine,
}

/// `getVendorSignature`: the vendor and the maximum basic leaf, or `None`
/// for `VendorSignatures::UNKNOWN`.
fn vendor_signature(cpuid: &impl Cpuid) -> Option<(Vendor, u32)> {
    let [max_leaf, ebx, ecx, edx] = get_cpuid(cpuid, 0, 0)?;
    if max_leaf < 1 {
        return None;
    }
    let vendor = match (ebx, edx, ecx) {
        // "Genu ineI ntel"
        (0x756e_6547, 0x4965_6e69, 0x6c65_746e) => Vendor::GenuineIntel,
        // "Auth enti cAMD"
        (0x6874_7541, 0x6974_6e65, 0x444d_4163) => Vendor::AuthenticAmd,
        // "Hygo nGen uine"
        (0x6f67_7948, 0x6e65_476e, 0x656e_6975) => Vendor::HygonGenuine,
        _ => return None,
    };
    Some((vendor, max_leaf))
}

/// `detectX86FamilyModel`: the display family and model of leaf 1's EAX.
const fn detect_family_model(eax: u32) -> (u32, u32) {
    let mut family = (eax >> 8) & 0xf;
    let mut model = (eax >> 4) & 0xf;
    if family == 6 || family == 0xf {
        if family == 0xf {
            family += (eax >> 20) & 0xff;
        }
        model += ((eax >> 16) & 0xf) << 4;
    }
    (family, model)
}

/// The `X86::FEATURE_*` bits `getAvailableFeatures` sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feature {
    Cmov,
    Mmx,
    Sse,
    Sse2,
    Sse3,
    Pclmul,
    Ssse3,
    Fma,
    Sse4_1,
    Sse4_2,
    Crc32,
    Popcnt,
    Aes,
    Movbe,
    Avx,
    Bmi,
    Avx2,
    Bmi2,
    Avx512f,
    Avx512dq,
    Adx,
    Avx512ifma,
    Clflushopt,
    Avx512cd,
    Sha,
    Avx512bw,
    Avx512vl,
    Avx512vbmi,
    Avx512vbmi2,
    Gfni,
    Vpclmulqdq,
    Avx512vnni,
    Avx512bitalg,
    Avx512vpopcntdq,
    Avx5124vnniw,
    Avx5124fmaps,
    Avx512vp2intersect,
    Avx512bf16,
    Sse4A,
    Xop,
    Fma4,
    Bit64,
}

/// A set of [`Feature`]s, upstream's `Features` bit array.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Features(u64);

impl Features {
    const fn has(self, feature: Feature) -> bool {
        self.0 & (1 << feature as u32) != 0
    }

    const fn set(&mut self, feature: Feature) {
        self.0 |= 1 << feature as u32;
    }

    /// Set each feature whose bit is set in `register`, when `gate` holds.
    fn set_bits(&mut self, register: u32, bits: &[(u32, Feature, Gate)], gates: Gates) {
        for &(bit, feature, gate) in bits {
            if (register >> bit) & 1 != 0 && gates.allow(gate) {
                self.set(feature);
            }
        }
    }
}

/// The OS-support condition a feature bit additionally needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    None,
    /// `HasAVX`: XCR0 saves the SSE and AVX state.
    Avx,
    /// `HasAVX512Save`: XCR0 also saves the opmask and ZMM state.
    Avx512Save,
}

#[derive(Debug, Clone, Copy)]
struct Gates {
    avx: bool,
    avx512_save: bool,
}

impl Gates {
    const fn allow(self, gate: Gate) -> bool {
        match gate {
            Gate::None => true,
            Gate::Avx => self.avx,
            Gate::Avx512Save => self.avx512_save,
        }
    }
}

/// Leaf 1 EDX.
const LEAF1_EDX: &[(u32, Feature, Gate)] = &[
    (15, Feature::Cmov, Gate::None),
    (23, Feature::Mmx, Gate::None),
    (25, Feature::Sse, Gate::None),
    (26, Feature::Sse2, Gate::None),
];

/// Leaf 1 ECX.
const LEAF1_ECX: &[(u32, Feature, Gate)] = &[
    (0, Feature::Sse3, Gate::None),
    (1, Feature::Pclmul, Gate::None),
    (9, Feature::Ssse3, Gate::None),
    (12, Feature::Fma, Gate::None),
    (19, Feature::Sse4_1, Gate::None),
    (20, Feature::Sse4_2, Gate::None),
    (20, Feature::Crc32, Gate::None),
    (23, Feature::Popcnt, Gate::None),
    (25, Feature::Aes, Gate::None),
    (22, Feature::Movbe, Gate::None),
];

/// Leaf 7 subleaf 0 EBX.
const LEAF7_EBX: &[(u32, Feature, Gate)] = &[
    (3, Feature::Bmi, Gate::None),
    (5, Feature::Avx2, Gate::Avx),
    (8, Feature::Bmi2, Gate::None),
    (16, Feature::Avx512f, Gate::Avx512Save),
    (17, Feature::Avx512dq, Gate::Avx512Save),
    (19, Feature::Adx, Gate::None),
    (21, Feature::Avx512ifma, Gate::Avx512Save),
    (23, Feature::Clflushopt, Gate::None),
    (28, Feature::Avx512cd, Gate::Avx512Save),
    (29, Feature::Sha, Gate::None),
    (30, Feature::Avx512bw, Gate::Avx512Save),
    (31, Feature::Avx512vl, Gate::Avx512Save),
];

/// Leaf 7 subleaf 0 ECX.
const LEAF7_ECX: &[(u32, Feature, Gate)] = &[
    (1, Feature::Avx512vbmi, Gate::Avx512Save),
    (6, Feature::Avx512vbmi2, Gate::Avx512Save),
    (8, Feature::Gfni, Gate::None),
    (10, Feature::Vpclmulqdq, Gate::Avx),
    (11, Feature::Avx512vnni, Gate::Avx512Save),
    (12, Feature::Avx512bitalg, Gate::Avx512Save),
    (14, Feature::Avx512vpopcntdq, Gate::Avx512Save),
];

/// Leaf 7 subleaf 0 EDX.
const LEAF7_EDX: &[(u32, Feature, Gate)] = &[
    (2, Feature::Avx5124vnniw, Gate::Avx512Save),
    (3, Feature::Avx5124fmaps, Gate::Avx512Save),
    (8, Feature::Avx512vp2intersect, Gate::Avx512Save),
];

/// Leaf 7 subleaf 1 EAX.
const LEAF7_1_EAX: &[(u32, Feature, Gate)] = &[(5, Feature::Avx512bf16, Gate::Avx512Save)];

/// Leaf `0x8000_0001` ECX.
const EXT_LEAF1_ECX: &[(u32, Feature, Gate)] = &[
    (6, Feature::Sse4A, Gate::None),
    (11, Feature::Xop, Gate::None),
    (16, Feature::Fma4, Gate::None),
];

/// Leaf `0x8000_0001` EDX.
const EXT_LEAF1_EDX: &[(u32, Feature, Gate)] = &[(29, Feature::Bit64, Gate::None)];

/// `getAvailableFeatures`, over leaf 1's ECX and EDX and the maximum basic
/// leaf. The `HasAVX512Save` arm is the non-Apple one.
fn available_features(cpuid: &impl Cpuid, ecx: u32, edx: u32, max_leaf: u32) -> Features {
    const AVX_BITS: u32 = (1 << 27) | (1 << 28);
    let mut features = Features::default();
    let ungated = Gates {
        avx: false,
        avx512_save: false,
    };
    features.set_bits(edx, LEAF1_EDX, ungated);
    features.set_bits(ecx, LEAF1_ECX, ungated);

    let xcr0 = ((ecx & AVX_BITS) == AVX_BITS).then(|| cpuid.xcr0());
    let has_avx = xcr0.is_some_and(|xcr0| xcr0 & 0x6 == 0x6);
    let gates = Gates {
        avx: has_avx,
        avx512_save: has_avx && xcr0.is_some_and(|xcr0| xcr0 & 0xe0 == 0xe0),
    };
    if has_avx {
        features.set(Feature::Avx);
    }

    if let Some([max_subleaf, ebx, ecx, edx]) =
        (max_leaf >= 7).then(|| get_cpuid(cpuid, 7, 0)).flatten()
    {
        features.set_bits(ebx, LEAF7_EBX, gates);
        features.set_bits(ecx, LEAF7_ECX, gates);
        features.set_bits(edx, LEAF7_EDX, gates);
        // Subleaf 0's EAX is the maximum subleaf; some CPUs do not zero the
        // invalid ones.
        if let Some([eax, ..]) = (max_subleaf >= 1).then(|| get_cpuid(cpuid, 7, 1)).flatten() {
            features.set_bits(eax, LEAF7_1_EAX, gates);
        }
    }

    let max_ext_leaf = get_cpuid(cpuid, 0x8000_0000, 0).map_or(0, |[eax, ..]| eax);
    if let Some([_, _, ecx, edx]) = (max_ext_leaf >= 0x8000_0001)
        .then(|| get_cpuid(cpuid, 0x8000_0001, 0))
        .flatten()
    {
        features.set_bits(ecx, EXT_LEAF1_ECX, gates);
        features.set_bits(edx, EXT_LEAF1_EDX, gates);
    }
    features
}

/// `getIntelProcessorTypeAndSubtype`.
const fn intel_cpu(family: u32, model: u32, features: Features) -> Option<&'static str> {
    match family {
        0x3 => Some("i386"),
        0x4 => Some("i486"),
        0x5 if features.has(Feature::Mmx) => Some("pentium-mmx"),
        0x5 => Some("pentium"),
        0x6 => Some(intel_family6(model, features)),
        0xf if features.has(Feature::Bit64) => Some("nocona"),
        0xf if features.has(Feature::Sse3) => Some("prescott"),
        0xf => Some("pentium4"),
        0x13 => match model {
            0x01 => Some("diamondrapids"),
            _ => None,
        },
        0x12 => match model {
            0x1 | 0x3 => Some("novalake"),
            _ => None,
        },
        _ => None,
    }
}

/// `getIntelProcessorTypeAndSubtype`'s family 6.
const fn intel_family6(model: u32, features: Features) -> &'static str {
    match model {
        0x0f | 0x16 => "core2",
        0x17 | 0x1d => "penryn",
        0x1a | 0x1e | 0x1f | 0x2e => "nehalem",
        0x25 | 0x2c | 0x2f => "westmere",
        0x2a | 0x2d => "sandybridge",
        0x3a | 0x3e => "ivybridge",
        0x3c | 0x3f | 0x45 | 0x46 => "haswell",
        0x3d | 0x47 | 0x4f | 0x56 => "broadwell",
        0x4e | 0x5e | 0x8e | 0x9e | 0xa5 | 0xa6 => "skylake",
        0xa7 => "rocketlake",
        0x55 if features.has(Feature::Avx512bf16) => "cooperlake",
        0x55 if features.has(Feature::Avx512vnni) => "cascadelake",
        0x55 => "skylake-avx512",
        0x66 => "cannonlake",
        0x7d | 0x7e => "icelake-client",
        0x8c | 0x8d => "tigerlake",
        0x97 | 0x9a => "alderlake",
        0xbe => "gracemont",
        0xb7 | 0xba | 0xbf => "raptorlake",
        0xaa | 0xac => "meteorlake",
        0xc5 | 0xb5 => "arrowlake",
        0xc6 => "arrowlake-s",
        0xbd => "lunarlake",
        0xcc => "pantherlake",
        0xd5 => "wildcatlake",
        0xad => "graniterapids",
        0xae => "graniterapids-d",
        0x6a | 0x6c => "icelake-server",
        0xcf => "emeraldrapids",
        0x8f => "sapphirerapids",
        0x1c | 0x26 | 0x27 | 0x35 | 0x36 => "bonnell",
        0x37 | 0x4a | 0x4d | 0x5a | 0x5d | 0x4c => "silvermont",
        0x5c | 0x5f => "goldmont",
        0x7a => "goldmont-plus",
        0x86 | 0x8a | 0x96 | 0x9c => "tremont",
        0xaf => "sierraforest",
        0xb6 => "grandridge",
        0xdd => "clearwaterforest",
        0x57 => "knl",
        0x85 => "knm",
        _ => intel_family6_guess(features),
    }
}

/// `getIntelProcessorTypeAndSubtype`'s guess for an unknown family-6 model.
const fn intel_family6_guess(features: Features) -> &'static str {
    if features.has(Feature::Avx512vp2intersect) {
        "tigerlake"
    } else if features.has(Feature::Avx512vbmi2) {
        "icelake-client"
    } else if features.has(Feature::Avx512vbmi) {
        "cannonlake"
    } else if features.has(Feature::Avx512bf16) {
        "cooperlake"
    } else if features.has(Feature::Avx512vnni) {
        "cascadelake"
    } else if features.has(Feature::Avx512vl) {
        "skylake-avx512"
    } else if features.has(Feature::Clflushopt) {
        if features.has(Feature::Sha) {
            "goldmont"
        } else {
            "skylake"
        }
    } else if features.has(Feature::Adx) {
        "broadwell"
    } else if features.has(Feature::Avx2) {
        "haswell"
    } else if features.has(Feature::Avx) {
        "sandybridge"
    } else if features.has(Feature::Sse4_2) {
        if features.has(Feature::Movbe) {
            "silvermont"
        } else {
            "nehalem"
        }
    } else if features.has(Feature::Sse4_1) {
        "penryn"
    } else if features.has(Feature::Ssse3) {
        if features.has(Feature::Movbe) {
            "bonnell"
        } else {
            "core2"
        }
    } else if features.has(Feature::Bit64) {
        "core2"
    } else if features.has(Feature::Sse3) {
        "yonah"
    } else if features.has(Feature::Sse2) {
        "pentium-m"
    } else if features.has(Feature::Sse) {
        "pentium3"
    } else if features.has(Feature::Mmx) {
        "pentium2"
    } else {
        "pentiumpro"
    }
}

/// `getAMDProcessorTypeAndSubtype`.
const fn amd_cpu(family: u32, model: u32, features: Features) -> Option<&'static str> {
    Some(match family {
        4 => "i486",
        5 => match model {
            6 | 7 => "k6",
            8 => "k6-2",
            9 | 13 => "k6-3",
            10 => "geode",
            _ => "pentium",
        },
        6 if features.has(Feature::Sse) => "athlon-xp",
        6 => "athlon",
        15 if features.has(Feature::Sse3) => "k8-sse3",
        15 => "k8",
        16 | 18 => "amdfam10",
        20 => "btver1",
        21 => match model {
            0x60..=0x7f => "bdver4",
            0x30..=0x3f => "bdver3",
            0x10..=0x1f | 0x02 => "bdver2",
            _ => "bdver1",
        },
        22 => "btver2",
        23 => match model {
            0x30..=0x3f
            | 0x47
            | 0x60..=0x67
            | 0x68..=0x6f
            | 0x70..=0x7f
            | 0x84..=0x87
            | 0x90..=0x97
            | 0x98..=0x9f
            | 0xa0..=0xaf => "znver2",
            _ => "znver1",
        },
        25 => match model {
            0x00..=0x0f | 0x20..=0x2f | 0x30..=0x3f | 0x40..=0x4f | 0x50..=0x5f => "znver3",
            0x10..=0x1f | 0x60..=0x6f | 0x70..=0x77 | 0x78..=0x7f | 0xa0..=0xaf => "znver4",
            _ => "znver3",
        },
        26 => match model {
            0x00..=0x4f | 0x60..=0x77 | 0xd0..=0xd7 => "znver5",
            0x50..=0x5f | 0x80..=0xcf | 0xd8..=0xe7 => "znver6",
            _ => "znver5",
        },
        _ => return None,
    })
}

/// `getHygonProcessorTypeAndSubtype`.
const fn hygon_cpu(family: u32, model: u32) -> Option<&'static str> {
    match (family, model) {
        (24, 4) => Some("c86-4g-m4"),
        (24, 6) => Some("c86-4g-m6"),
        (24, 7) => Some("c86-4g-m7"),
        (24, 8) => Some("c86-4g-m8"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded leaves; an unrecorded leaf reads as zeros.
    struct Recorded {
        leaves: Vec<((u32, u32), [u32; 4])>,
        xcr0: u64,
    }

    impl Recorded {
        /// This repository's build host, an i7-10875H (Comet Lake,
        /// family 6 model `0xa5`).
        fn comet_lake() -> Self {
            Self {
                leaves: vec![
                    ((0, 0), [0x16, 0x756e_6547, 0x6c65_746e, 0x4965_6e69]),
                    ((1, 0), [0x000a_0652, 0x0610_0800, 0x7ffa_fbff, 0xbfeb_fbff]),
                    ((7, 0), [0, 0x029c_67af, 0x4000_0018, 0xbc00_0600]),
                    ((7, 1), [0, 0, 0, 0]),
                    ((0x8000_0000, 0), [0x8000_0008, 0, 0, 0]),
                    ((0x8000_0001, 0), [0, 0, 0x121, 0x2c10_0800]),
                ],
                xcr0: 0x21f,
            }
        }

        fn with(mut self, leaf: (u32, u32), registers: [u32; 4]) -> Self {
            match self.leaves.iter_mut().find(|(key, _)| *key == leaf) {
                Some(entry) => entry.1 = registers,
                None => self.leaves.push((leaf, registers)),
            }
            self
        }

        fn with_leaf1_eax(self, eax: u32) -> Self {
            let [_, ebx, ecx, edx] = self.cpuid(1, 0);
            self.with((1, 0), [eax, ebx, ecx, edx])
        }

        fn with_vendor(self, [ebx, edx, ecx]: [u32; 3]) -> Self {
            let max_leaf = self.cpuid(0, 0)[0];
            self.with((0, 0), [max_leaf, ebx, ecx, edx])
        }
    }

    impl Cpuid for Recorded {
        fn cpuid(&self, leaf: u32, subleaf: u32) -> [u32; 4] {
            self.leaves
                .iter()
                .find(|(key, _)| *key == (leaf, subleaf))
                .map_or([0; 4], |(_, registers)| *registers)
        }

        fn xcr0(&self) -> u64 {
            let ecx = self.cpuid(1, 0)[2];
            assert_eq!(ecx & (3 << 27), 3 << 27, "XGETBV read without OSXSAVE");
            self.xcr0
        }
    }

    const AMD: [u32; 3] = [0x6874_7541, 0x6974_6e65, 0x444d_4163];
    const HYGON: [u32; 3] = [0x6f67_7948, 0x6e65_476e, 0x656e_6975];
    const CENTAUR: [u32; 3] = [0x746e_6543, 0x4872_7561, 0x736c_7561];

    #[test]
    fn known_intel_models_name_their_cpu() {
        assert_eq!(cpu_name(&Recorded::comet_lake()), "skylake");
        assert_eq!(
            cpu_name(&Recorded::comet_lake().with_leaf1_eax(0x000a_0672)),
            "rocketlake"
        );
    }

    #[test]
    fn amd_and_hygon_families_name_their_cpu() {
        // Family 0xf + 0x19, model 0x61 (Raphael).
        let raphael = Recorded::comet_lake()
            .with_vendor(AMD)
            .with_leaf1_eax(0x00a6_0f11);
        assert_eq!(cpu_name(&raphael), "znver4");
        // Family 0xf + 0x9, model 4.
        let hygon = Recorded::comet_lake()
            .with_vendor(HYGON)
            .with_leaf1_eax(0x0090_0f40);
        assert_eq!(cpu_name(&hygon), "c86-4g-m4");
    }

    #[test]
    fn unknown_vendors_and_missing_leaves_are_generic() {
        assert_eq!(
            cpu_name(&Recorded::comet_lake().with_vendor(CENTAUR)),
            "generic"
        );
        let no_leaves =
            Recorded::comet_lake().with((0, 0), [0, 0x756e_6547, 0x6c65_746e, 0x4965_6e69]);
        assert_eq!(cpu_name(&no_leaves), "generic");
    }

    #[test]
    fn unknown_family6_models_are_guessed_from_features() {
        // Family 6, model 0xff.
        let unknown = Recorded::comet_lake().with_leaf1_eax(0x000f_06f0);
        assert_eq!(cpu_name(&unknown), "skylake");
        // BMI, AVX2, and BMI2, without ADX or CLFLUSHOPT.
        let avx2 = unknown.with((7, 0), [0, 0x0000_0128, 0, 0]);
        assert_eq!(cpu_name(&avx2), "haswell");
        // XCR0 without the AVX state withholds AVX and AVX2; SSE4.2 with
        // MOVBE remains.
        let no_avx_state = Recorded { xcr0: 0x3, ..avx2 };
        assert_eq!(cpu_name(&no_avx_state), "silvermont");
    }

    #[test]
    fn xcr0_is_read_only_behind_osxsave_and_avx() {
        // Leaf 1 ECX without OSXSAVE: reading the recorded XCR0 would panic,
        // and AVX2 stays withheld.
        let [_, ebx, ecx, edx] = Recorded::comet_lake().cpuid(1, 0);
        let no_osxsave = Recorded::comet_lake()
            .with((1, 0), [0x000f_06f0, ebx, ecx & !(1 << 27), edx])
            .with((7, 0), [0, 0x0000_0128, 0, 0]);
        assert_eq!(cpu_name(&no_osxsave), "silvermont");
    }
}
