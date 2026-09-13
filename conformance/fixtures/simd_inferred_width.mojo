# Mojito infers an unbound SIMD width from the number of element arguments;
# upstream has no constructor that does, so the placeholder stays unbound.
def main():
    var v: SIMD[DType.int, 4] = SIMD[DType.int, _](1, 2, 3, 4)
    print(v[2])
