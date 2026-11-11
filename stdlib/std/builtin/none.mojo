# Upstream's `std/builtin/none.mojo`: `struct NoneType`, which wraps the
# `!kgen.none` type of the `None` expression. The prelude exports it, as
# upstream's does. `None` converts to it through the `@implicit`
# initializer; the raw `None` has no members and conforms to no trait.
#
# The `@implicit` initializer spells `` __mlir_type.`!kgen.none` `` where
# upstream writes `Self._mlir_type`: the library's comptime members convert
# `None` before the struct's own `comptime` members resolve (R2007).


struct NoneType(
    Defaultable,
    ImplicitlyCopyable,
    TrivialRegisterPassable,
    Writable,
):
    comptime _mlir_type = __mlir_type.`!kgen.none`

    var _value: Self._mlir_type

    @always_inline("builtin")
    def __init__(out self):
        self._value = None

    @always_inline("builtin")
    @implicit
    def __init__(out self, value: __mlir_type.`!kgen.none`):
        self._value = value

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        writer.write_string("None")

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write_string("None")
