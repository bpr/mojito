# Mojito's subset of upstream `std/format/_utils.mojo`: the repr vocabulary
# `write_repr_to` bodies build on. `TypeNames` writes a type pack's
# unqualified names, `FormatStruct` writes `Name[params](fields)` through
# upstream's fluent chain (`FormatStruct(writer, "Name").params(...)
# .fields(...)`), and `Named` writes `name=value`. Not ported: the
# `fields[FieldsFn]` callback overload (nested `@parameter def`), `Repr`, and
# the free `write_to`/`write_repr_to`. The bundled collections'
# `write_repr_to` bodies write their text directly through
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
# and returns the builder for chaining; `fields` writes the parenthesized
# field list. Each element is written through `Writer.write`.
struct FormatStruct[T: Writer, o: Origin[mut=True]](Movable):
    var _writer: Pointer[Self.T, Self.o]

    def __init__(out self, ref[Self.o] writer: Self.T, var name: String):
        writer.write(name)
        self._writer = Pointer(to=writer)

    def params[*Ts: Writable](self, *args: *Ts) -> ref[self] Self:
        self._writer[].write("[")
        comptime for i in range(Ts.length):
            if i > 0:
                self._writer[].write(", ")
            self._writer[].write(args[i])
        self._writer[].write("]")
        return self

    def fields[*Ts: Writable](self, *args: *Ts):
        self._writer[].write("(")
        comptime for i in range(Ts.length):
            if i > 0:
                self._writer[].write(", ")
            self._writer[].write(args[i])
        self._writer[].write(")")


# Upstream's `name=value` wrapper: a reference to the value, not a copy.
struct Named[T: Writable, o: Origin[mut=False]](Copyable, Movable, Writable):
    var _name: String
    var _value: Pointer[Self.T, Self.o]

    def __init__(out self, var name: String, ref[Self.o] value: Self.T):
        self._name = name^
        self._value = Pointer(to=value)

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self._name, "=", self._value[])
