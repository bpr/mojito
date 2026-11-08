# A compile-time operator over `SIMD` operands is its dunder's call, as
# Mojo's parser spells `v // w` as `v.__floordiv__(w)`: a builtin dunder
# folds to the operator node, and any other (`__floordiv__`, `__mod__`) is
# the call's application, so each pair below is one type. A float literal
# operand converts through `SIMD.__init__` in both spellings.


struct S[x: Int32](Movable):
    def __init__(out self):
        pass


struct V[x: SIMD[DType.float32, 2]](Movable):
    def __init__(out self):
        pass


struct B[x: Bool](Movable):
    def __init__(out self):
        pass


def int32s[v: Int32, w: Int32]() -> S[v // w]:
    var md: S[v % w] = S[v.__mod__(w)]()
    var lt: B[v < w] = B[v.__lt__(w)]()
    _ = md^
    _ = lt^
    return S[v.__floordiv__(w)]()


def vectors[v: SIMD[DType.float32, 2]]():
    var mul: V[v * 2.0] = V[v.__mul__(2.0)]()
    var eq: B[v == 2.0] = B[v.__eq__(2.0)]()
    _ = mul^
    _ = eq^


def main():
    _ = int32s[7, 2]()
    vectors[SIMD[DType.float32, 2](2.0)]()
    comptime k: Int32 = 7
    print(S[k // 2].x, S[k % 2].x, B[k < 2].x)
