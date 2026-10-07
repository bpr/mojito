# A `comptime for` over tuple or struct elements in a generic body is one
# loop in the template, as at the pin: its variable is a compile-time
# parameter of the element type, and each read materializes that constant. A
# literal display, a display over a binder, a module or local list, and a
# pack-keyed `def` all keep the loop; the variable is passed whole, copied,
# subscripted, read by field and method, and tested in a `comptime if`. A
# tuple-typed value parameter reads the same way. Output matches the pin.


@fieldwise_init
struct P(Copyable, ImplicitlyCopyable, Movable):
    var a: Int
    var b: Int

    def get(self) -> Int:
        return self.a * 100 + self.b


@fieldwise_init
struct Q(Copyable, ImplicitlyCopyable, Movable):
    var x: Float64
    var on: Bool


@fieldwise_init
struct R(Copyable, ImplicitlyCopyable, Movable):
    var q: Q
    var k: Int


comptime PAIRS = [(1, 2), (3, 4)]
comptime PS = [P(1, 2), P(3, 4)]


def h(t: Tuple[Int, Int]) -> Int:
    return t[0] * 10 + t[1]


def k(p: P):
    print("k", p.a, p.b)


def generic[n: Int]():
    comptime for p in [(1, 2), (3, n)]:
        print(p[0] + p[1], h(p))
        var q = p
        print(q[1])
    comptime for p in PAIRS:
        print(p[0] * n, p[1])
    comptime for p in [P(1, n), P(n, 2)]:
        print(p.a, p.b, p.get())
        k(p)
    comptime for p in PS:
        print(p.a + n, p.b)
    comptime for p in [((1, 2), 3.5, True), ((n, 4), 0.25, False)]:
        print(p[0][0] + p[0][1], p[1], p[2])
    comptime for r in [R(Q(1.5, True), n), R(Q(2.5, False), 7)]:
        print(r.q.x, r.q.on, r.k)
    comptime L = [(1, n), (n, 2)]
    comptime for p in L:
        comptime if p[0] > 1:
            print("big", p[0])
        else:
            print("small", p[0])
    comptime x = L[0][1]
    print(x)


struct S[n: Int]:
    @staticmethod
    def show():
        comptime for p in [(Self.n, 1), (2, Self.n)]:
            print(p[0] * p[1])


def packed[*Ts: Writable](*args: *Ts):
    comptime for p in [(1, 2), (3, 4)]:
        print(p[0], len(args))


def tuple_parameter[p: Tuple[Int, Int]]():
    print(p[0], p[1], h(p))


def main():
    generic[10]()
    generic[2]()
    S[7].show()
    packed(1, "x")
    tuple_parameter[(5, 6)]()
