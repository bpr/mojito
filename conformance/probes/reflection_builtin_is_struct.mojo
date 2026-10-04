# PROBE: every type Mojito names is a struct to the pin's reflection.
#
# `Reflected[T].is_struct()` is `#kgen.is_struct_type<T>`, `False` only for
# an MLIR primitive (`__mlir_type.index`), which no Mojito program spells:
# a builtin scalar, `String`, `NoneType`, a vector, `DType`, a function's
# type, and a field handle over an `Int` field all answer `True`. Both
# compilers print the `True` lines.
#
# The trailing `field_count()` line is the pin's alone: it prints `1 1 1 3`
# (`Int`, `Float64`, `Bool`, `String`), while Mojito rejects a non-struct
# operand ("requires a struct type"; roadmap R76).
#
# Observed 2026-10-04 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflection_builtin_is_struct.mojo
#         cargo run -- run conformance/probes/reflection_builtin_is_struct.mojo
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


def main():
    print(reflect[Int].is_struct(), reflect[UInt].is_struct())
    print(reflect[Bool].is_struct(), reflect[Float64].is_struct())
    print(reflect[String].is_struct(), reflect[NoneType].is_struct())
    print(reflect[SIMD[DType.int32, 4]].is_struct(), reflect[DType].is_struct())
    print(reflect[Point].field["y"].is_struct())
    print(
        reflect[Int].field_count(),
        reflect[Float64].field_count(),
        reflect[Bool].field_count(),
        reflect[String].field_count(),
    )
