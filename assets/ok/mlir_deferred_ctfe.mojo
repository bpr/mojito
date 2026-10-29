# A function returning `!kgen.deferred` runs at compile time: the
# elaborator's VM holds the attribute it returns as a value, which an
# instance's parameter then binds.
from std.simd import FastMathFlag


struct F[a: __mlir_type.`!kgen.deferred`]:
    @staticmethod
    def x() -> Int:
        return 1


def attr() -> __mlir_type.`!kgen.deferred`:
    return __mlir_attr.`#pop.fmf<fast>`


def pick[b: Bool]() -> __mlir_type.`!kgen.deferred`:
    if b:
        return __mlir_attr.`#pop.fmf<fast>`
    return __mlir_attr.`#pop.fmf<none>`


def wrap() -> __mlir_type.`!kgen.deferred`:
    return pick[True]()


def k[flag: FastMathFlag]() -> Int:
    comptime f = flag._mlir_attr()
    return F[f].x() + 1


def main():
    comptime a = attr()
    print(F[attr()].x(), F[a].x(), F[wrap()].x(), F[pick[False]()].x())
    print(F[FastMathFlag.FAST._mlir_attr()].x(), k[FastMathFlag.NNAN]())
