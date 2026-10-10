# A local `comptime` display, a `Dict`/`Set` constructor binding, and a
# closed reflection list are each an application the check binds, read
# whole by a lifted call or a `comptime(e)` operand as well as by a loop,
# a condition, or `materialize`.
from std.collections import Set
from std.hashlib.hasher import default_hasher


def total[n: Int](xs: Array[Int, n]) -> Int:
    var s = 0
    for i in range(n):
        s += xs[i]
    return s


def size(d: Dict[String, Int]) -> Int:
    return len(d)


def count(s: Set[Int]) -> Int:
    return len(s)


@fieldwise_init
struct P:
    var x: Int
    var y: Int


comptime D = Dict[String, Int, default_hasher](["a", "b"], [1, 2], None)
comptime DN = size(D)
comptime E = Dict[String, Int](["a", "b", "c"], [1, 2, 3], None)
comptime EN = size(E)
comptime S = Set[Int, default_hasher](1, 2, 3)
comptime SN = count(S)
comptime NAMES = reflect[P].field_names()
comptime SECOND = NAMES[1]


def generic_body[k: Int]():
    comptime l = [1, 2, 3]
    comptime s = total(l)
    print(s + k)


def outer():
    def inner():
        comptime d = {"k": 7}
        comptime g = d.get("k").value()
        print(g)

    inner()


def main():
    comptime l = [1, 2, 3]
    comptime for x in l:
        print(x)
    comptime if l[0] == 1:
        print("first is 1")
    comptime q = l[2]
    print(q)
    print(materialize[l]())
    comptime s = total(l)
    print(s)
    print(comptime(total(l)))
    comptime k = l
    comptime for y in k:
        print(y)

    comptime d = {"a": 1, "b": 2}
    comptime g = d.get("b").value()
    print(g)
    comptime n = size(d)
    print(n)
    comptime if "a" in d:
        print("has a")
    print(len(materialize[d]()))

    comptime LD = Dict[String, Int, default_hasher](["a", "b"], [1, 2], None)
    comptime ln = size(LD)
    print(ln)
    comptime if "b" in LD:
        print("has b")

    print(DN, EN, SN)
    comptime if 2 in S:
        print("has 2")
    print(len(materialize[D]()))

    comptime names = reflect[P].field_names()
    comptime for name in names:
        print(name)
    comptime if names[0] == "x":
        print("x first")
    comptime for name in NAMES:
        print(name)
    print(materialize[names]()[1], SECOND)

    comptime w = "abc"
    comptime wl = w.byte_length()
    print(wl)

    generic_body[10]()
    outer()
