# The casts to `index` keep the polynomials they cast apart: `k + 1` and
# `k * 2` stay two types, as at the pin.
# expect: type mismatch for variable 'x': expected S[(k + 1).__mlir_index__()], found S[(2 * k).__mlir_index__()]
@fieldwise_init
struct S[n: __mlir_type.index](Copyable, Movable):
    pass


def h[k: Int]():
    var x: S[(k + 1).__mlir_index__()] = S[(k * 2).__mlir_index__()]()
    _ = x^


def main():
    h[1]()
