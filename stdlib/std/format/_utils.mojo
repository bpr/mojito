# Mojito's subset of upstream `std/format/_utils.mojo`: the repr vocabulary
# `write_repr_to` bodies build on. `TypeNames` writes a type pack's
# unqualified names and `FormatStruct` writes `Name[params](fields)`.
# Divergences from upstream: `FormatStruct.params`/`fields` take `mut self`
# and `params` returns nothing, so the builder is bound to a local and
# called step by step (`var f = FormatStruct(writer, "Name")`;
# `f.params(...)`; `f.fields(...)`) rather than chained on a temporary; the
# `fields[FieldsFn]` callback overload, `Named`, `Repr`, and the free
# `write_to`/`write_repr_to` are not ported (a `Named` temporary holding a
# pointer to a caller local reads a stale frame on the VM). The bundled
# collections' `write_repr_to` bodies write their text directly through
# `_unqualified_type_name` rather than the builder, so an instance mints no
# builder or `TypeNames` specialization of its own.

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


# Upstream's builder for `Name[param, ...](field, ...)` representations. The
# constructor writes the name; `params` writes the bracketed parameter list
# and `fields` the parenthesized field list, each element through `Writer.write`.
struct FormatStruct[T: Writer, o: Origin[mut=True]](Movable):
    var _writer: Pointer[Self.T, Self.o]

    def __init__(out self, ref[Self.o] writer: Self.T, var name: String):
        writer.write(name)
        self._writer = Pointer(to=writer)

    def params[*Ts: Writable](mut self, *args: *Ts):
        self._writer[].write("[")
        comptime for i in range(Ts.length):
            if i > 0:
                self._writer[].write(", ")
            self._writer[].write(args[i])
        self._writer[].write("]")

    def fields[*Ts: Writable](mut self, *args: *Ts):
        self._writer[].write("(")
        comptime for i in range(Ts.length):
            if i > 0:
                self._writer[].write(", ")
            self._writer[].write(args[i])
        self._writer[].write(")")
