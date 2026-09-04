# Compile-time type reflection (current Mojo's `std.reflection.type_info`).
# `_unqualified_type_name[T]()` is a compiler intrinsic: the linker exports
# the name as an identity from this module home and the checker folds each
# resolution to the unqualified spelling of the checked type
# (`SIMD[DType.int, 1]` for `Int`, `Optional[String]`, `Tuple[Int, Bool]`,
# a user struct's bare name). The module holds no definitions of its own.
