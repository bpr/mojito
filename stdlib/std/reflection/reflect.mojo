"""Unified compile-time reflection over a type: `reflect[T]`.

`reflect[T]` is a compiler builtin available implicitly in every program, as
upstream's prelude imports it; this module is its canonical import home. The
linker exports the builtin identity from this docstring-only file so explicit
`from std.reflection import reflect` spellings resolve. Upstream's
`Reflected[T]` handle type it aliases is not bundled (R324).
"""
