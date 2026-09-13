# Upstream splats a `Bool` across a mask with the `fill` keyword, since `Bool`
# is not a `Scalar` and so does not take the positional splat; Mojito has no
# `fill` argument and rejects the call.
def main():
    print(SIMD[DType.bool, 4](fill=True))
