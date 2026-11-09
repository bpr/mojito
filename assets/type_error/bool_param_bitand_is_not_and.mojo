# expect: type mismatch
# A compile-time `b & c` is `Bool.__and__`'s `pop.simd.and`, and `b and c`
# the selection `c if b else b`: two different parameter expressions, so
# their types differ even where their values agree.


struct Tag[b: Bool](ImplicitlyCopyable):
    def __init__(out self):
        pass


def f[b: Bool, c: Bool](x: Tag[b & c]) -> Tag[b and c]:
    return x


def main():
    _ = f[True, True](Tag[True]())
