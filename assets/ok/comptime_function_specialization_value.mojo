# A bracket over a function read as a compile-time value is the symbol of
# its specialization, typed by the instance's signature: a module or local
# `comptime` binds it, with or without a type argument or an annotation, a
# call through the binding, run-time or compile-time, calls the instance, a
# run-time copy holds it, and a callable parameter takes the binding. An
# annotation selects one declaration of an overload set.


def twice[n: Int]() -> Int:
    return 2 * n


def add[n: Int](x: Int) -> Int:
    return x + n


def tagged[T: AnyType, v: Int]() -> Int:
    return v


def pick(x: Int) -> Int:
    return x * 10


def pick(x: String) -> Int:
    return 1


def apply[F: def() thin -> Int]() -> Int:
    return F()


comptime g = twice[4]
comptime a3 = add[3]
comptime t: def() thin -> Int = twice[6]
comptime seven = tagged[Int, 7]
comptime p: def(Int) thin -> Int = pick


def main():
    print(g())
    print(a3(4))
    print(t())
    print(seven())
    print(p(3))
    print(apply[g]())
    comptime r = a3(5)
    print(r)
    var k = a3
    print(k(10))
    comptime h = twice[5]
    print(h())
    comptime s = add[2]
    print(s(1))
