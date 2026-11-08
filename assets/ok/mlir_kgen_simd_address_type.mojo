# Upstream's KGEN vector of addresses `!kgen.simd<size, address>`, the
# base-pointer operand `masked_gather` and `masked_scatter` pass to
# `llvm.masked.gather` and `llvm.masked.scatter`, spelled through a
# `SIMDLength` binder's `_mlir_value` in a generic body, at four lanes and
# one.


def lanes[size: SIMDLength]() -> Int:
    comptime V = __mlir_type[`!kgen.simd<`, size._mlir_value, `, address>`]
    return Int(size)


def main():
    print(lanes[4]())
    print(lanes[1]())
