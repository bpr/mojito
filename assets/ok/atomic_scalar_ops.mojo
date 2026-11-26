# `std.atomic` as at the pin: `Atomic`'s read-modify-write, compare-exchange,
# and min/max over `Int`, `Float64`, and a wrapping `UInt8`; explicit
# orderings, `unordered` and weak forms, a scope, and `fence`; `Ordering`'s
# formatting; the static forms over a pointer; and a compile-time call,
# whose interpreter arm reads and writes plainly, agreeing with the run.
from std.atomic import Atomic, Ordering, fence


def counted(n: Int) -> Int:
    var a = Atomic[Int](0)
    for i in range(n):
        _ = a.fetch_add(i)
    a -= 1
    var e = a.load()
    _ = a.compare_exchange(e, e * 2)
    return a.load()


def main():
    var a = Atomic[Int](10)
    print(a.fetch_sub(3))
    print(a.load())
    a.store(42)
    print(a.load())
    var e = 42
    print(a.compare_exchange(e, 7), a.load())
    a += 5
    a -= 2
    print(a.load())
    a.max(100)
    a.min(50)
    print(a.load())

    var f = Atomic[Float64](1.5)
    print(f.fetch_add(2.0))
    f.max(10.0)
    var fe: Float64 = 10.0
    print(f.compare_exchange(fe, 3.0), f.load())
    var u = Atomic[UInt8](250)
    print(u.fetch_add(10), u.load())

    var b = Atomic[Int](1)
    b.store[ordering=Ordering.RELAXED](5)
    print(b.load[ordering=Ordering.ACQUIRE]())
    fence()
    fence[Ordering.ACQUIRE]()
    print(Ordering.RELAXED, repr(Ordering.SEQUENTIAL))
    var s = Atomic[Int, scope="singlethread"](3)
    print(s.fetch_sub(1))

    var w = Atomic[Int](1)
    print(w.load[ordering=Ordering.UNORDERED]())
    w.store[ordering=Ordering.UNORDERED](4)
    print(w.load[ordering=Ordering.RELAXED]())
    var we = 4
    print(
        w.compare_exchange[
            success_ordering=Ordering.RELAXED,
            failure_ordering=Ordering.RELAXED,
            weak=True,
        ](we, 9),
        w.load(),
    )

    comptime c = counted(5)
    print(c, counted(5))
    var x = 7
    print(Atomic[Int].fetch_add(Pointer(to=x), 3), x)
    print(Atomic[Int].load(Pointer(to=x)))
