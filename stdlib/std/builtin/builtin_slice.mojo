# `Slice`, `ContiguousSlice`, `StridedSlice`, and the `slice(...)` constructor
# are compiler-provided descriptor types: slice literals select the contiguous
# or strided kind, explicit `Slice(...)`/`slice(...)` construction builds the
# general one, and `indices`, equality, and `Slice(start, end, step)` writing
# are intrinsic. This docstring-only module is current Mojo's import home
# (`from std.builtin.builtin_slice import ContiguousSlice`); the linker
# exports the names as identities.
