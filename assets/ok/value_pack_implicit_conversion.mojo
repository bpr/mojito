# Each element of a value pack converts through the element type's
# `@implicit` constructor, and the call carries the converted elements for
# the elaborator: `f[1, 2]()` over `*xs: W`, `cnt["a", "b"]()` over
# `*extra: StaticString`, and a pack spread into `_get_kgen_string`.
from std.collections.string.string_span import _get_kgen_string


struct W(ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, v: IntLiteral):
        self.v = Int(v)


def f[*xs: W]() -> Int:
    return len(xs)


def cnt[*extra: StaticString]() -> Int:
    return len(extra)


def popcount[*parts: StaticString](x: Int64) -> Int64:
    comptime k = _get_kgen_string["llvm.", *parts]()
    return __mlir_op.`pop.call_llvm_intrinsic`[
        intrin=k,
        _type=Int64,
        hasSideEffects=False.__mlir_i1__(),
    ](x)


def main():
    print(f[1, 2]())
    print(cnt["a", "b"]())
    print(popcount["ct", "pop"](Int64(7)))
