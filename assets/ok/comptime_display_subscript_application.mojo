# A subscript of a list or tuple display in a compile-time position is the
# display's subscript application, evaluated below the check, not a fold over
# a frozen element list: a module constant, a local tuple element, and a
# fieldwise element loaded out of the reference result without a destructor.
@fieldwise_init
struct D(Movable):
    var n: Int

    def __deinit__(deinit self):
        print("deinit", self.n)


@fieldwise_init
struct P(Copyable, Movable):
    var a: Int
    var b: String


comptime M = [5, 6, 7][2]


def g[k: Int]() -> Int:
    return k


def main():
    print(M)
    print(g[[8, 9][1]]())
    comptime t = (1, "a")[1]
    print(t)
    comptime r = [D(1), D(2)][1]
    print(r.n)
    comptime q = [P(1, "x"), P(2, "y")][0]
    print(q.a, q.b)
    comptime s = (P(3, "z"), 4)[0]
    print(s.b)
