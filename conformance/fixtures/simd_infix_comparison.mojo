# Upstream's `SIMD.__lt__` asserts `Self.length == 1`, pointing at
# `SIMD.lt(...)` for the mask, so a strict inequality over a four-lane
# vector fails the instance that reaches it; `==` compares whole vectors
# to a `Bool`.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    var w = SIMD[DType.int32, 4](25, 25, 25, 25)
    print(v < w, v == w)
