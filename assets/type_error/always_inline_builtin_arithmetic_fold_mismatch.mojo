# expect: expected S[v * w], found S[v + w]
# `SIMD.__add__`'s builtin body folds to `v + w`, which is not `v * w`: the
# folded dunder is the operator it names, not an operator equal to all.
struct S[x: Int32](Movable):
    def __init__(out self):
        pass


def f[v: Int32, w: Int32]() -> S[v * w]:
    return S[v.__add__(w)]()


def main():
    _ = f[2, 2]()
