# expect: type mismatch
# A compile-time `b | ~b` keeps both operands: the parameter folder finds
# a complement under `&` (`b & ~b` is `False`) but not under `|`, so it
# is not the constant `True`.


struct Tag[b: Bool](ImplicitlyCopyable):
    def __init__(out self):
        pass


def f[b: Bool](x: Tag[b | ~b]) -> Tag[True]:
    return x


def main():
    _ = f[True](Tag[True]())
