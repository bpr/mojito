# Mojito's infix comparisons are elementwise at every width, so `<` on a wide
# SIMD builds a mask and `==` compares lane by lane. Upstream constrains the
# strict inequalities to `Scalar` and gives `==`/`!=` whole-vector meaning,
# pointing at `SIMD.lt(...)` and friends for the mask.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    var w = SIMD[DType.int32, 4](25, 25, 25, 25)
    print(v < w, v == w)
