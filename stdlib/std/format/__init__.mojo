"""Formatted text output (subset of upstream's `format`): the `Writer` and
`Writable` traits, with `Writable`'s reflective default bodies,
`_reflection_write_to`, and `_enum_reflection_write_to` verbatim; its names
are imported explicitly rather than taken from the prelude."""

from std.builtin.constrained import _field_conforms_to_error
from std.builtin.enum_like import EnumLike
from std.builtin.variadics import ParameterList, TypeList
from std.reflection import reflect
from std.reflection.type_info import _unqualified_type_name
from std.string import StaticString, StringSpan
from std.range import range


trait Writer:
    """A destination for formatted text output: `write_string` is the core
    method, and `write` writes each `Writable` argument in turn."""

    def write_string(mut self, string: StringSpan):
        ...

    def write[*Ts: Writable](mut self, *args: *Ts):
        comptime for i in range(args.__len__()):
            args[i].write_to(self)


trait Writable:
    """A trait for types that can format themselves as text.

    The `Writable` trait provides a simple, straightforward interface for types
    that need to convert themselves to text. Types implementing `Writable` write
    directly to a `Writer`, making formatting efficient and allocation-free.

    Both `write_to()` and `write_repr_to()` have default implementations that
    use reflection to automatically format all fields. This means simple structs
    can conform to `Writable` without any method implementations:

    ```mojo
    @fieldwise_init
    struct Point(Writable):
        var x: Int
        var y: Int

    var p = Point(1, 3)
    print(p)       # Point(x=1, y=3)
    print(repr(p)) # Point(x=Int(1), y=Int(3))
    ```

    Override either for different normal and debug output:

    ```mojo
    @fieldwise_init
    struct Point(Writable):
        var x: Float64
        var y: Float64

        def write_to(self, mut writer: Some[Writer]):
            writer.write("(", self.x, ", ", self.y, ")")

        def write_repr_to(self, mut writer: Some[Writer]):
            writer.write("Point: x=", self.x, ", y=", self.y)

    var p = Point(1.5, 2.7)
    print(p)       # (1.5, 2.7)
    print(repr(p)) # Point: x=1.5, y=2.7
    ```

    Note: The default reflection-based implementations iterate over all fields
    at compile time. For mutually recursive types (e.g., struct `A` has a field
    of type `List[B]` and struct `B` has a field of type `A`), this creates an
    infinite monomorphization cycle that causes the compiler to hang. To fix
    this, provide explicit `write_to()` and `write_repr_to()` implementations
    for at least one type in the cycle.
    """

    def write_to(self, mut writer: Some[Writer]):
        """Write this value's text representation to a writer.

        This method is called by `print()`, `String()`, and format strings to
        convert the value to text. Override this method to define how your type
        appears when printed or converted to a string.

        The default implementation uses reflection to format all fields as
        `TypeName(field1=value1, field2=value2, ...)`, calling `write_to()`
        on each field. All fields must conform to `Writable`. `EnumLike` types
        are instead formatted as `TypeName.case(payload)`, or `TypeName.case`
        when the active case's payload is `NoneType`.

        Args:
            writer: The destination for formatted output.

        **Example:**

        ```mojo
        @fieldwise_init
        struct Point(Writable):
            var x: Float64
            var y: Float64

            def write_to(self, mut writer: Some[Writer]):
                writer.write("(", self.x, ", ", self.y, ")")
        ```
        """
        comptime WriterType = type_of(writer)

        @inline(.always)
        def call_write_to[
            FieldType: Writable
        ](field: FieldType, mut writer: WriterType):
            field.write_to(writer)

        _reflection_write_to[f=call_write_to](self, writer)

    def write_repr_to(self, mut writer: Some[Writer]):
        """Write this value's debug representation to a writer.

        This method is called by `repr(value)` or the `"{!r}"` format specifier
        and should produce unambiguous, developer-facing output that shows the
        internal state of the value.

        The default implementation uses reflection to format all fields as
        `TypeName(field1=value1, field2=value2, ...)`, calling `write_repr_to()`
        on each field. All fields must conform to `Writable`. `EnumLike` types
        are instead formatted as `TypeName.case(payload)`, or `TypeName.case`
        when the active case's payload is `NoneType`.

        Args:
            writer: The destination for formatted output.

        **Example:**

        ```mojo
        @fieldwise_init
        struct Point(Writable):
            var x: Float64
            var y: Float64

            def write_repr_to(self, mut writer: Some[Writer]):
                writer.write("Point: x=", self.x, ", y=", self.y)
        ```

        Notes:
            Mojo's repr always prints single quotes (`'`) at the start and end
            of the repr. Any single quote inside a string should be escaped
            (`\\'`).
        """

        comptime WriterType = type_of(writer)

        @inline(.always)
        def call_write_repr_to[
            FieldType: Writable
        ](field: FieldType, mut writer: WriterType):
            field.write_repr_to(writer)

        _reflection_write_to[f=call_write_repr_to](self, writer)


@inline(.always)
def _reflection_write_to[
    T: Writable,
    W: Writer,
    //,
    f: def[FieldType: Writable](field: FieldType, mut writer: W) thin,
](this: T, mut writer: W,):
    comptime if conforms_to(T, EnumLike):
        _enum_reflection_write_to[f=f](this, writer)
        return

    comptime r = reflect[T]
    comptime names = r.field_names()
    comptime types = r.field_types()
    comptime type_name = _unqualified_type_name[T]()
    writer.write_string(type_name)
    writer.write_string("(")

    comptime for i in range(names.length):
        comptime FieldType = types[i]
        comptime assert conforms_to(
            FieldType, Writable
        ), _field_conforms_to_error[
            Parent=T,
            FieldIndex=i,
            ParentConformsTo="Writable",
        ]()

        comptime if i > 0:
            writer.write_string(", ")
        writer.write_string(materialize[names[i]]())
        writer.write_string("=")

        ref field = r.field_ref[i](this)
        f(field, writer)

    writer.write_string(")")


@inline(.always)
def _enum_reflection_write_to[
    T: EnumLike & Writable,
    W: Writer,
    //,
    f: def[FieldType: Writable](field: FieldType, mut writer: W) thin,
](this: T, mut writer: W):
    """Writes `TypeName.case(payload)`, or `TypeName.case` for `NoneType`
    payloads."""
    comptime names = ParameterList[T._enum_case_names]()
    comptime types = TypeList[Trait=AnyType, T._enum_case_types]()
    comptime type_name = _unqualified_type_name[T]()
    writer.write_string(type_name)
    writer.write_string(".")

    var discriminant = this._get_enum_discriminant()
    comptime for i in range(names.length):
        if discriminant == i:
            writer.write_string(StaticString(names[i]))

            comptime PayloadType = types[i]
            comptime if PayloadType != NoneType:
                comptime assert conforms_to(PayloadType, Writable), (
                    "the payload of every case must conform to `Writable` to"
                    " use the default `Writable` implementation for an"
                    " `EnumLike` type"
                )
                writer.write_string("(")
                f(this._unsafe_get_enum_payload[i](), writer)
                writer.write_string(")")
            return
