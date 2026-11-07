# Upstream's `std/sys/defines.mojo`, in part: `is_defined`,
# `_is_bool_like`, `get_defined_bool`, both `get_defined_int`s, and both
# `get_defined_string`s, verbatim, over `#kgen.param.expr<get_env>`. The
# compiler takes no `-D` yet (R1981), so no name is defined, as under a
# `mojo run` given none.
# Omitted: `get_defined_dtype`, over the unbundled `DType._from_str`
# (R1978), and `MojoVersion` with `MOJO_VERSION`, over the
# `lit.mojo.version` ops (R1982).

"""Implements functions for retrieving compile-time defines.

You can use these functions to set parameter values or runtime constants based on
name-value pairs defined on the command line. For example:

```mojo
  from std.sys import is_defined

  comptime float_type = DType.float32 if is_defined["FLOAT32"]() else DType.float64

  # Use `float_type` as a constant.
```

And on the command line:

```
  mojo -D FLOAT32 main.mojo
```

For more information, see the [Mojo build docs](https://mojolang.org/docs/cli/build/#d-keyvalue).
The `mojo run` command also supports the `-D` option.


You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import is_defined
```
"""

from std.collections.string.string_span import _get_kgen_string
from std.string import StaticString


def is_defined[name: StaticString]() -> Bool:
    """Return true if the named value is defined.

    Parameters:
        name: The name to test.

    Returns:
        True if the name is defined.
    """
    return __mlir_attr[
        `#kgen.param.expr<get_env, `,
        _get_kgen_string[name](),
        `> : i1`,
    ]


def _is_bool_like[val: StaticString]() -> Bool:
    return (
        val == "0"
        or val == "1"
        or val == "true"
        or val == "True"
        or val == "TRUE"
        or val == "false"
        or val == "False"
        or val == "FALSE"
        or val == "on"
        or val == "On"
        or val == "ON"
        or val == "off"
        or val == "Off"
        or val == "OFF"
    )


def get_defined_bool[name: StaticString, default: Bool = False]() -> Bool:
    """Get a boolean-valued define. If the name is not defined, return a default
    value instead. If the name is defined this will return `True` if the value
    is ("1", "true", "True", "TRUE", "on", "On", "ON") otherwise it will return
    `False`.

    Parameters:
        name: The name of the define.
        default: The default value to use.

    Returns:
        A boolean parameter value.
    """
    comptime if is_defined[name]():
        comptime val = get_defined_string[name]()
        return (
            val == "1"
            or val == "true"
            or val == "True"
            or val == "TRUE"
            or val == "on"
            or val == "On"
            or val == "ON"
        )
    else:
        return default


def get_defined_int[name: StaticString]() -> Int:
    """Try to get an integer-valued define. Compilation fails if the
    name is not defined.

    Parameters:
        name: The name of the define.

    Returns:
        An integer parameter value.
    """
    return Int(
        mlir_value=__mlir_attr[
            `#kgen.param.expr<get_env, `,
            _get_kgen_string[name](),
            `> : index`,
        ]
    )


def get_defined_int[name: StaticString, default: Int]() -> Int:
    """Try to get an integer-valued define. If the name is not defined,
    return a default value instead.

    Parameters:
        name: The name of the define.
        default: The default value to use.

    Returns:
        An integer parameter value.

    Example:
    ```mojo
    from std.sys.defines import get_defined_int

    def main() raises:
        comptime number = get_defined_int[
            "favorite_number",
            1 # Default value
        ]()
        parametrized[number]()

    def parametrized[num: Int]():
        print(num)
    ```

    If the program is `app.mojo`:
    - `mojo run -D favorite_number=2 app.mojo`
    - `mojo run -D app.mojo`

    Note: useful for parameterizing SIMD vector sizes.
    """

    comptime if is_defined[name]():
        return get_defined_int[name]()
    else:
        return default


def get_defined_string[name: StaticString]() -> StaticString:
    """Try to get a string-valued define. Compilation fails if the
    name is not defined.

    Parameters:
        name: The name of the define.

    Returns:
        A string parameter value.
    """
    var res = __mlir_attr[
        `#kgen.param.expr<get_env, `,
        _get_kgen_string[name](),
        `> : !kgen.string`,
    ]
    return StaticString(res)


def get_defined_string[
    name: StaticString, default: StaticString
]() -> StaticString:
    """Try to get a string-valued define. If the name is not defined,
    return a default value instead.

    Parameters:
        name: The name of the define.
        default: The default value to use.

    Returns:
        A string parameter value.
    """

    comptime if is_defined[name]():
        return get_defined_string[name]()
    else:
        return default
