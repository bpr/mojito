# `llvm_intrinsic` calls each LLVM intrinsic Mojito computes, on scalars
# and vectors, at run time and at compile time, and the
# `pop.call_llvm_intrinsic` op it is spelled over takes plain operands.
from std.sys import llvm_intrinsic


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 7)
    print(llvm_intrinsic["llvm.ctpop", SIMD[DType.int32, 4], has_side_effect=False](v))
    print(llvm_intrinsic["llvm.ctlz", Int64, has_side_effect=False](Int64(1), False))
    print(llvm_intrinsic["llvm.ctlz", Int, has_side_effect=False](1, False))
    print(
        llvm_intrinsic["llvm.cttz", SIMD[DType.uint8, 4], has_side_effect=False](
            SIMD[DType.uint8, 4](1, 8, 96, 128), False
        )
    )
    print(llvm_intrinsic["llvm.bitreverse", Int8, has_side_effect=False](Int8(1)))
    print(llvm_intrinsic["llvm.bswap", UInt16, has_side_effect=False](UInt16(0x1234)))
    print(
        llvm_intrinsic["llvm.bswap", SIMD[DType.int32, 2], has_side_effect=False](
            SIMD[DType.int32, 2](1, 256)
        )
    )
    print(
        llvm_intrinsic["llvm.fshl", UInt8, has_side_effect=False](
            UInt8(0x81), UInt8(0x81), UInt8(9)
        )
    )
    print(llvm_intrinsic["llvm.fshl", Int, has_side_effect=False](1, 2, 3))
    print(
        llvm_intrinsic["llvm.fshr", SIMD[DType.uint8, 2], has_side_effect=False](
            SIMD[DType.uint8, 2](1, 2), SIMD[DType.uint8, 2](128, 4), SIMD[DType.uint8, 2](1, 2)
        )
    )
    print(
        llvm_intrinsic["llvm.copysign", Float64, has_side_effect=False](
            Float64(2.5), Float64(-1.0)
        )
    )
    print(
        llvm_intrinsic["llvm.sqrt", SIMD[DType.float32, 2], has_side_effect=False](
            SIMD[DType.float32, 2](2.25, 16.0)
        )
    )
    print(llvm_intrinsic["llvm.vector.reduce.smax", Int32, has_side_effect=False](v))
    print(llvm_intrinsic["llvm.vector.reduce.smin", Int32, has_side_effect=False](v))
    var u = SIMD[DType.uint8, 4](1, 200, 3, 4)
    print(llvm_intrinsic["llvm.vector.reduce.umax", UInt8, has_side_effect=False](u))
    print(llvm_intrinsic["llvm.vector.reduce.smax", UInt8, has_side_effect=False](u))
    print(llvm_intrinsic["llvm.vector.reduce.umin", UInt8, has_side_effect=False](u))
    print(llvm_intrinsic["llvm.vector.reduce.and", Int32, has_side_effect=False](v))
    print(llvm_intrinsic["llvm.vector.reduce.or", Int32, has_side_effect=False](v))
    var f = SIMD[DType.float32, 4](1.0, 9.5, 3.0, 4.0)
    print(llvm_intrinsic["llvm.vector.reduce.fmax", Float32, has_side_effect=False](f))
    print(llvm_intrinsic["llvm.vector.reduce.fmin", Float32, has_side_effect=False](f))
    var zero = Float64(0.0)
    var n = SIMD[DType.float64, 2](zero / zero, 2.0)
    print(llvm_intrinsic["llvm.vector.reduce.fmax", Float64, has_side_effect=False](n))
    llvm_intrinsic["llvm.donothing", NoneType]()

    comptime c = llvm_intrinsic["llvm.ctpop", Int32, has_side_effect=False](Int32(7))
    print(c)

    var x = Int32(7)
    print(
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin="llvm.ctpop".value,
            _type=Int32,
            hasSideEffects=False.__mlir_i1__(),
        ](x)
    )
    print(
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin="llvm.ctlz".value,
            _type=Int32,
            hasSideEffects=False.__mlir_i1__(),
        ](Int32(1), False.__mlir_i1__())
    )
