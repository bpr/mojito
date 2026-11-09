# expect: type mismatch
# A compile-time `b ^ b` is `pop.simd.xor` over the same storage twice,
# which the parameter folder keeps: it is not the constant `False`.


struct Tag[b: Bool](ImplicitlyCopyable):
    def __init__(out self):
        pass


def f[b: Bool](x: Tag[b ^ b]) -> Tag[False]:
    return x


def main():
    _ = f[True](Tag[False]())
