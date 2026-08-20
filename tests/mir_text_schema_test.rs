//! Textual MIR schema vocabulary and compatibility pins.

use mojito::Ty;
use mojito::mir::text::{
    INSTRUCTION_MNEMONICS, MAGIC, RESERVED_WORDS, TERMINATOR_MNEMONICS, TYPE_SPELLINGS,
    VERSION_MAJOR, VERSION_MINOR, constant_spelling, instruction_mnemonic,
    intrinsic_subscript_spelling, is_bare_identifier, projection_spelling, quote,
    terminator_mnemonic, type_spelling, use_mode_spelling, version_header,
};
use mojito::mir::{Const, MirInstr, MirIntrinsicSubscript, MirTerm, Proj, Reg, UseMode};
use std::collections::HashSet;

#[test]
fn schema_version_and_envelope_are_stable() {
    assert_eq!(MAGIC, "mojito-mir");
    assert_eq!((VERSION_MAJOR, VERSION_MINOR), (1, 0));
    assert_eq!(version_header(), "mojito-mir 1.0");
}

#[test]
fn executable_and_reserved_spellings_are_unique() {
    let mut seen = HashSet::new();
    for spelling in INSTRUCTION_MNEMONICS
        .iter()
        .chain(TERMINATOR_MNEMONICS)
        .chain(RESERVED_WORDS)
    {
        assert!(
            seen.insert(spelling),
            "duplicate schema spelling: {spelling}"
        );
        assert!(spelling.is_ascii());
        assert!(!spelling.is_empty());
    }
}

#[test]
fn closed_supporting_enums_have_canonical_spellings() {
    assert_eq!(use_mode_spelling(UseMode::Copy), "copy");
    assert_eq!(use_mode_spelling(UseMode::Move), "move");
    assert_eq!(use_mode_spelling(UseMode::BorrowShared), "borrow_shared");
    assert_eq!(use_mode_spelling(UseMode::BorrowMut), "borrow_mut");

    assert_eq!(projection_spelling(&Proj::Field("x".into())), "field");
    assert_eq!(projection_spelling(&Proj::Index(Reg(0))), "index");
    assert_eq!(projection_spelling(&Proj::ConstIndex(0)), "const_index");
    assert_eq!(projection_spelling(&Proj::Variant(0)), "variant");
    assert_eq!(projection_spelling(&Proj::UninitPayload), "uninit_payload");

    let intrinsics = [
        (MirIntrinsicSubscript::TupleStorage, "tuple_storage"),
        (MirIntrinsicSubscript::VariadicStorage, "variadic_storage"),
        (MirIntrinsicSubscript::Simd, "simd"),
        (MirIntrinsicSubscript::Pointer, "pointer"),
        (MirIntrinsicSubscript::ComptimeList, "comptime_list"),
    ];
    for (intrinsic, spelling) in intrinsics {
        assert_eq!(intrinsic_subscript_spelling(intrinsic), spelling);
    }
}

#[test]
fn representative_instruction_and_terminator_mappings_are_pinned() {
    assert_eq!(
        instruction_mnemonic(&MirInstr::KeepAlive { var: 0 }),
        "lifetime.keep_alive"
    );
    assert_eq!(
        instruction_mnemonic(&MirInstr::Unsupported("future".into())),
        "unsupported"
    );
    assert_eq!(terminator_mnemonic(&MirTerm::Jump(2)), "jump");
    assert_eq!(terminator_mnemonic(&MirTerm::FallOff), "falloff");
}

#[test]
fn constants_and_types_use_schema_tags() {
    assert_eq!(constant_spelling(&Const::None), "none");
    assert_eq!(
        constant_spelling(&Const::Function("main".into())),
        "function"
    );
    assert_eq!(type_spelling(&Ty::Int), "Int");
    assert_eq!(type_spelling(&Ty::Tuple(vec![Ty::Int, Ty::Bool])), "tuple");
}

#[test]
fn type_spellings_are_canonical_and_unique() {
    let mut seen = HashSet::new();
    for spelling in TYPE_SPELLINGS {
        assert!(seen.insert(spelling), "duplicate type spelling: {spelling}");
        assert!(spelling.is_ascii());
        assert!(!spelling.is_empty());
    }
    for representative in [
        type_spelling(&Ty::Int),
        type_spelling(&Ty::Tuple(vec![Ty::Int])),
        type_spelling(&Ty::Variant(vec![Ty::Int])),
    ] {
        assert!(
            TYPE_SPELLINGS.contains(&representative),
            "type spelling missing from the inventory: {representative}"
        );
    }
}

#[test]
fn artifact_strings_do_not_depend_on_mojo_literal_syntax() {
    assert!(is_bare_identifier("field_name2"));
    assert!(!is_bare_identifier("fn"));
    assert!(!is_bare_identifier("a.b"));
    assert_eq!(
        quote("line\n\"slash\\\u{1}"),
        "\"line\\n\\\"slash\\\\\\u{1}\""
    );
}
