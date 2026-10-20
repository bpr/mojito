# `Slice`, `ContiguousSlice`, `StridedSlice`, and the `slice(...)` constructor
# are compiler-provided descriptor types: a slice literal with a step value
# selects the strided kind and one without the contiguous kind, explicit
# `Slice(...)`/`slice(...)` construction builds the general one, and
# `indices`, equality, and `Slice(start, end, step)` writing are intrinsic.
# This docstring-only module is current Mojo's home for all four: `Slice` and
# `slice` are prelude names, and the two kinds are visible only where an
# import binds them (`from std.builtin.builtin_slice import ContiguousSlice`),
# which the linker gives a qualified spelling.
