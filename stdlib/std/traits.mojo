"""Core object-lifetime and value-semantics traits.

`AnyType`, `Movable`, `Copyable`, `ImplicitlyCopyable`, `Deinitable`, and the
`IsTrivially*` comptime predicates are compiler builtins available implicitly in
every program; this module is their canonical import home. The linker exports
the builtin identities from this docstring-only file so explicit
`from std.traits import ...` spellings resolve.
"""
