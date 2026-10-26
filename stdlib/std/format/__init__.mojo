"""Formatted text output (subset of upstream's `format`): the `Writer` and
`Writable` traits. A conformer without a `write_to` or `write_repr_to` taking
a `Writer` gets upstream's reflective default as a synthesized member, which
writes each field through `_reflection_write_field_to` (R1461)."""

from std.string import StringSpan
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
    """A type that formats itself as text into a `Writer`: `write_to` for
    `print`, `String()`, and format strings, `write_repr_to` for `repr`."""

    def write_to(self, mut writer: Some[Writer]):
        ...

    def write_repr_to(self, mut writer: Some[Writer]):
        ...


def _reflection_write_field_to[
    FieldType: AnyType, //, is_repr: Bool
](field: FieldType, mut writer: Some[Writer]):
    """Write one field of a reflectively written conformer, as upstream's
    `_reflection_write_to` does: the field's type must be `Writable`, checked
    where the default is instantiated."""
    comptime assert conforms_to(
        FieldType, Writable
    ), "Could not derive Writable - a member field does not implement Writable"
    comptime if is_repr:
        field.write_repr_to(writer)
    else:
        field.write_to(writer)
