# Mojito's subset of upstream `std/format/_utils.mojo`: the repr vocabulary
# `write_repr_to` bodies build on. `TypeNames` writes a type pack's
# unqualified names. Upstream's `FormatStruct` builder
# (`FormatStruct(writer, "Name").params(...).fields(...)`), `Repr`, `Named`,
# and the free `write_repr_to`/`write_to` are not ported: the builder's
# `params`/`fields` are variadic-pack methods on a struct, which the
# elaborator does not specialize yet, and the bundled `Writable` trait
# declares only `write_to` (see docs/roadmap.md).

from std.reflection.type_info import _unqualified_type_name
from std.string import String


# The comma-separated unqualified names of a type pack: `TypeNames[Int,
# String]()` writes `SIMD[DType.int, 1], String`. The names are cut out of
# the pack's `Tuple[...]` spelling (a pack spread expands per
# specialization, where a variadic struct's bare `Self` would not).
struct TypeNames[*Ts: Movable](ImplicitlyCopyable, Movable, Writable):
    var _unused: Int

    def __init__(out self):
        self._unused = 0

    def write_to(self, mut writer: Some[Writer]):
        var full = String(_unqualified_type_name[Tuple[*Ts]]())
        # "Tuple[" is six bytes; the closing bracket is the last one.
        var names = full[byte=6:full.byte_length() - 1]
        writer.write(names)
