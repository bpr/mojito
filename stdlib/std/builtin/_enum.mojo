# Upstream's `std/builtin/_enum.mojo`: the runtime half of `enum` (the pin's
# `__enum`). The compiler declares an enum's members against this module, and
# nothing in it is user-facing.
#
# For
#
#     enum Token:
#         case eof
#         case ident(String)
#         case span(Int, Int)
#
# the compiler generates, written as Mojo:
#
#     struct Token(__Enum):
#         comptime _enum_case_names = ParameterList.of[
#             "eof".value, "ident".value, "span".value
#         ].values
#         comptime _enum_case_types = TypeList.of[
#             Trait=AnyType, NoneType, String, Tuple[Int, Int]
#         ].values
#         var __enum_storage: __EnumStorage[NoneType, String, Tuple[Int, Int]]
#
#         comptime eof: Token = Token.__enum_init(
#             __EnumStorage[NoneType, String, Tuple[Int, Int]](None, index=0)
#         )
#
#         @staticmethod
#         def ident(var payload: String) -> Token: ...
#         @staticmethod
#         def span(var payload0: Int, var payload1: Int) -> Token: ...
#
# with `__enum_init`, `__get_enum_storage`, `_get_enum_discriminant`, and
# `_unsafe_get_enum_payload` generated on the enum itself
# (`comptime::synthesize_enum_members`). Upstream declares those four as
# `__Enum`'s defaults and `__EnumStorage` as a struct over `!kgen.variant`;
# here `__EnumStorage[*Ts]` is the compiler's case-indexed storage, the same
# intrinsic `Variant`'s `__VariantStorage` is, and `__Enum` is the
# conformance every enum declares (`docs/notes/enums.md`).
"""The runtime half of `enum`."""

from std.builtin.enum_like import EnumLike


@doc_hidden
def __enum_discriminant_dtype(num_cases: Int) -> DType:
    """Returns the `!kgen.variant` discriminant type for `num_cases` cases."""
    if num_cases <= 256:
        return DType.uint8
    if num_cases <= 65536:
        return DType.uint16
    return DType.uint32


trait __Enum(EnumLike):
    """An `__enum`: a struct whose only field, `__enum_storage`, is an
    `__EnumStorage` over its case types."""

    pass
