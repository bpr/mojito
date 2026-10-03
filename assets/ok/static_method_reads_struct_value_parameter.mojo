# A `@staticmethod` of a value-parameterized struct reads its struct's
# parameter as `Self.k`, in an expression and in a bracket slot from which a
# callee infers its own value parameter (`Counter[Self.k]`), and spells it in
# a static call's receiver (`W[Self.k].st()`). The pinned Mojo prints 5, 35,
# 111, 8, 6, 5, 7.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i


def size[n: Int](c: Counter[n]) -> Int:
    return n


struct W[k: Int]:
    def __init__(out self):
        pass

    @staticmethod
    def st() -> Int:
        return Self.k

    @staticmethod
    def plus(a: Int, b: Int) -> Int:
        return a + b + Self.k

    @staticmethod
    def chain(a: Int) -> Int:
        return W[Self.k].plus(a, 1) + W[Self.k].st()

    @staticmethod
    def sized(a: Int) -> Int:
        return size(Counter[Self.k](a)) + a

    def inst(self) -> Int:
        return W[Self.k].st() + W[Self.k].st()


def raising() raises -> Int:
    return W[7].st()


def main():
    print(W[5].st())
    print(W[5].plus(10, 20))
    print(W[5].chain(100))
    print(W[5].sized(3))
    print(W[3]().inst())
    try:
        print(W[4].sized(1))
        print(raising())
    except e:
        print(e)
