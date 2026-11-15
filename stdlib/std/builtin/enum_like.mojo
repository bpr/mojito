# The `SumType`, `EnumLike`, and `UnionLike` traits of upstream's enum
# proposal (`Mojo/proposals/enums.md`), in upstream's
# `std/builtin/enum_like.mojo`. The prelude exports all three.
"""Defines the `SumType` family of traits used by case-pattern matching."""

from std.builtin.variadics import TypeList, _MLIR


comptime KGENString = __mlir_type.`!kgen.string`
"""The MLIR string type used for case names."""


trait SumType:
    """A type with named, mutually exclusive alternatives.

    Every `enum` and `union` conforms. Hand-written types may also conform so
    pattern matching can treat their alternatives as cases.

    By default, the declared alternatives cover every value. An open sum type
    sets `_enum_is_exhaustive` to `False` when other values may exist.
    """

    comptime _sum_case_names: _MLIR.KGENParamListType[KGENString]
    """Case names in discriminant order."""

    comptime _enum_is_exhaustive: Bool = True
    """Whether the declared cases cover every value of the conforming type."""

    def _get_sum_discriminant(self) -> Int:
        """Return the active case index."""
        ...


trait EnumLike(SumType):
    """A sum type whose cases carry no payload. Every `enum` conforms."""

    pass


trait UnionLike(SumType):
    """A sum type whose every case carries one payload. Every `union`
    conforms, and so may a hand-written type (notably `Optional`), whose
    cases then match as `case .Some(x):` / `case .None():`.
    """

    comptime _union_case_types: _MLIR.KGENParamListType[AnyType]
    """Payload type for each case, in discriminant order."""

    comptime _union_elt_type_for_case[id: Int]: AnyType = TypeList[
        Trait=AnyType, Self._union_case_types
    ]()[id]
    """Payload type for case `id`."""

    # FIXME: Prefer an interior origin so payload refs preserve subject
    # mutability cleanly. Return type uses TypeList directly (not
    # `_union_elt_type_for_case`) to avoid a recursive alias cycle.
    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        """Return a reference to the payload of case `id`.

        Only valid when `_get_sum_discriminant()` equals `id`.
        """
        ...
