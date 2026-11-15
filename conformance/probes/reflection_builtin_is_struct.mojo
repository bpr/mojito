# PROBE: every type Mojito names is a struct to the pin's reflection.
#
# `Reflected[T].is_struct()` is `#kgen.is_struct_type<T>`: a builtin scalar,
# `String`, `NoneType`, a vector, `DType`, a function's type, and a field
# handle over an `Int` field all answer `True`. A scalar is `SIMD`'s
# application, so its `field_count()` is `SIMD`'s one `_mlir_value` field.
# Both compilers print every line, the last `1 1 1 3` (`Int`, `Float64`,
# `Bool`, `String`).
#
# A spelled MLIR type (`__mlir_type.index`) also answers `True` at the pin,
# while the same `!kgen.scalar<index>` reached as `reflect[Int]`'s field
# type answers `False`, where Mojito answers `True`.
#
# Observed 2026-10-04 against `Mojo 1.2.0.dev2026092105 (e9569894)`; the
# `field_count()` line re-observed 2026-11-14 against
# `Mojo 1.3.0.dev2026110105`.
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
