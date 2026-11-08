# PROBE: a three-lane vector whose value survives to code generation.
#
# Found while landing R1734 (2026-11-08). The pin (`Mojo
# 1.3.0.dev2026110105`) admits `SIMD[DType.int32, 3]` and constructs it, but
# its LLVM lowering rejects the surviving `!kgen.simd<3, si32>`: "SIMD vector
# length must be a power of two between 1 and 2^15". Mojito prints
# `[2, 4, 6]`.
def main():
    var w = SIMD[DType.int32, 3](1, 2, 3)
    print(w + w)
