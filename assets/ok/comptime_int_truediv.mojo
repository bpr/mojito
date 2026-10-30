# A compile-time `/` over machine integers truncates as the run-time
# `pop.div` does, in a binding, a value parameter's body, a type argument,
# and a `where` clause; an `IntLiteral` pair still divides exactly.
struct W[n: Int]:
    @staticmethod
    def show():
        print("W", Self.n)


@fieldwise_init
struct B[n: Int](ImplicitlyCopyable):
    pass


def half[n: Int]() -> Int:
    comptime h = n / 2
    return h


def wide[n: Int]() where n / 2 > 1:
    print("wide", n)


def halved[n: Int](b: B[n / 2]):
    print("halved", n)


def main():
    comptime k = Int(-9) / 2
    comptime u = UInt(7) / UInt(2)
    comptime v = SIMD[DType.int32, 2](7, -7) / SIMD[DType.int32, 2](2, 2)
    comptime m = 7 / Int(2)
    comptime l = 7 / 2
    print(k, u, v, m, l)
    print(half[7](), half[-7]())
    W[half[9]()].show()
    W[k].show()
    wide[5]()
    halved[7](B[3]())
