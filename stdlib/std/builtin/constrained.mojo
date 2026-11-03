# Upstream's `std/builtin/constrained.mojo`, in part:
# `_field_conforms_to_error`, verbatim, with `StaticString` imported
# explicitly rather than taken from the prelude.
# Omitted: `_constrained_conforms_to` (R1881).
"""Implements compile-time constraint helpers used by trait conformance checks.
"""
from std.collections.string.string_span import _get_kgen_string
from std.reflection import reflect
from std.reflection.type_info import _unqualified_type_name
from std.string import StaticString


@inline(.nodebug)
def _field_conforms_to_error[
    *,
    Parent: AnyType,
    FieldIndex: Int,
    ParentConformsTo: StaticString,
    FieldConformsTo: StaticString = ParentConformsTo,
]() -> StaticString:
    """Asserts that a struct field conforms to a trait at compile time.

    This helper is used in default trait implementations that use reflection
    to operate on all fields of a struct. It produces a clear error message
    when a field doesn't conform to the required trait.

    Parameters:
        Parent: The struct type being checked.
        FieldIndex: The index of the field in the struct.
        ParentConformsTo: The trait the parent is trying to conform to.
        FieldConformsTo: The trait the field must conform to
            (defaults to ParentConformsTo).
    """
    comptime r = reflect[Parent]
    comptime names = r.field_names()
    comptime field_name = names[FieldIndex]
    comptime parent_type_name = _unqualified_type_name[Parent]()
    comptime types = r.field_types()
    comptime FieldType = types[FieldIndex]

    # Construct a message like:
    #     Could not derive Equatable for Point - member field `x: Int`
    #     does not implement Equatable
    # For MLIR types, omit the type name since `_unqualified_type_name`
    # can't handle non-struct types.
    comptime if reflect[FieldType].is_struct():
        comptime field_type_name = _unqualified_type_name[FieldType]()
        return StaticString(
            _get_kgen_string[
                "Could not derive ",
                ParentConformsTo,
                " for ",
                parent_type_name,
                " - member field `",
                field_name,
                ": ",
                field_type_name,
                "` does not implement ",
                FieldConformsTo,
            ]()
        )
    else:
        return StaticString(
            _get_kgen_string[
                "Could not derive ",
                ParentConformsTo,
                " for ",
                parent_type_name,
                " - member field `",
                field_name,
                "` does not implement ",
                FieldConformsTo,
            ]()
        )
