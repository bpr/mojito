# expect: not an associated comptime expression
# `Bool(mlir_value=s)` creates a `Bool` from its `!kgen.scalar<bool>`
# storage; a `Bool` is not one, so a compile-time `Bool(mlir_value=b)`
# over `b: Bool` applies no initializer, as at run time.


struct Tag[b: Bool](ImplicitlyCopyable):
    def __init__(out self):
        pass


def f[b: Bool](x: Tag[Bool(mlir_value=b)]):
    print(b)


def main():
    f[True](Tag[True]())
