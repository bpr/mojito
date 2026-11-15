# Upstream's `std/builtin/_enum.mojo`: the runtime half of `enum` and
# `union` (upstream's enum proposal, `Mojo/proposals/enums.md`; the pin's
# `__enum`). The compiler declares their members against this module, and
# nothing in it is user-facing.
#
# For
#
#     union Token:
#         case eof()
#         case ident(String)
#         case span(Int, Int)
#
# the compiler generates, written as Mojo:
#
#     struct Token(__Union):
#         comptime _enum_case_names = ParameterList.of[
#             "eof".value, "ident".value, "span".value
#         ].values
#         comptime _enum_case_types = TypeList.of[
#             Trait=AnyType, Tuple[], String, Tuple[Int, Int]
#         ].values
#         var __enum_storage: __EnumStorage[Tuple[], String, Tuple[Int, Int]]
#
#         @staticmethod
#         def eof() -> Token: ...
#         @staticmethod
#         def ident(var payload0: String) -> Token: ...
#         @staticmethod
#         def span(var payload0: Int, var payload1: Int) -> Token: ...
#
# and an `enum`'s cases, which carry no payload, are `comptime` values of
# `NoneType` storage instead (`comptime red: Color = Color.__enum_init(
# __EnumStorage[NoneType, ...](None, index=0))`), declaring `__Enum`.
# Both get `__enum_init`, `__get_enum_storage`, `_get_enum_discriminant`,
# and `_unsafe_get_enum_payload` generated on the declaration itself
# (`mojito_parser::enums`). Upstream declares those four as `__Enum`'s
# defaults and `__EnumStorage` as a struct over `!kgen.variant`; here
# `__EnumStorage[*Ts]` is the compiler's case-indexed storage, the same
# intrinsic `Variant`'s `__VariantStorage` is (`docs/notes/enums.md`).
"""The runtime half of `enum` and `union`."""

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
    """An `enum` or `union`: a struct whose only field, `__enum_storage`, is an
    `__EnumStorage` over its case types."""

    pass


trait __Union(__Enum):
    """A `union`: an `__Enum` whose every case carries one payload, so its
    case patterns take their payload subpatterns as a tuple pattern."""

    pass
