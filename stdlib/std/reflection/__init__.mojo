"""Compile-time reflection utilities for introspecting Mojo types and functions.

`reflect` is auto-imported via the prelude; this package root re-exports it
for explicit `from std.reflection import reflect` spellings. The rest of
upstream's root — `Reflected`, `ReflectedFn`, `reflect_fn`,
`SourceLocation`, `source_location`, `call_location`, `get_linkage_name`,
and `get_function_name` — is not bundled (R324).
"""

from .reflect import reflect
