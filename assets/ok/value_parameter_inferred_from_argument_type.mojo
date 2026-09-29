# A `def` value parameter is inferred from an argument's type, as the pinned
# Mojo infers it: `size(Counter[4](1))` binds `n = 4` against
# `def size[n: Int](c: Counter[n])`. The inferred value reaches the erased
# body whether the parameter is ordinary or infer-only, forwarded from an
# enclosing binder or an expression over one, read off a struct's own
# parameter, taken inside an unrolled loop, or declared by a nested `def`.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i


def size[n: Int](c: Counter[n]) -> Int:
    return n


def size_infer_only[n: Int, //](c: Counter[n]) -> Int:
    return n


def outer[m: Int](c: Counter[m]) -> Int:
    return size(c) + 100


def shifted[m: Int](c: Counter[m]) -> Int:
    return size(Counter[m + 1](1)) * 100 + size(Counter[2 * m - 3](1))


def labelled[T: Writable, n: Int](x: T, c: Counter[n]) -> Int:
    print(x)
    return n


struct Holder[k: Int]:
    def __init__(out self):
        pass

    def measure(self) -> Int:
        return size(Counter[9](1))

    def own(self) -> Int:
        return size(Counter[Self.k](1))


def main():
    print(size(Counter[4](1)))
    print(size[4](Counter[4](1)))
    print(size_infer_only(Counter[5](1)))
    print(outer(Counter[6](1)))
    print(labelled("eight", Counter[8](1)))
    print(shifted(Counter[6](1)))
    print(Holder[3]().measure())
    print(Holder[3]().own())
    comptime for i in range(2):
        print(size(Counter[i](1)))

    def inner[k: Int](c: Counter[k]) -> Int:
        return k * 2

    print(inner(Counter[10](1)))
