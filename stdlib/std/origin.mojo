"""Origin (provenance) vocabulary.

`Origin`, `OriginSet`, and the tracked/untracked/unsafe origin spellings are
compiler builtins available implicitly in every program; this module is their
canonical import home. The linker exports the builtin identities from this
docstring-only file so explicit `from std.origin import ...` spellings resolve.
"""
