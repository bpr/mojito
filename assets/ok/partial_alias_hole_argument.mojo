# A partial alias applied to `_` in a signature leaves that hole to the
# argument, as the bare alias does: `S[_]` over `comptime S = V[_,
# length=1]`, and the prelude's `Scalar[_]`.
struct V[dtype: DType, length: Int](Copyable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


comptime S = V[_, length=1]


def held(x: S[_]) -> Int:
    return x.x + 1


def bare(x: S) -> Int:
    return x.x + 3


def widened(x: Scalar[_]) -> Int:
    return Int(x) + 1


def main():
    print(held(V[DType.int8, 1](4)), bare(V[DType.int8, 1](4)))
    print(widened(Int8(4)))
