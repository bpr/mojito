# Two variadic overloads that differ in the regular parameters before the
# collector, tied on conversions. The pinned Mojo prefers, in order: fewer
# implicit copies into a `var` parameter, more arguments bound by value, fewer
# `ref` parameters. A read parameter of a trivially register-passable type
# binds by value, and so does a `var` parameter taking an rvalue. A literal a
# pack absorbs costs what its materialization costs a regular parameter.
# requires: discovery


def count[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def count[*Ts: Writable](a: Int, b: Int, *rest: *Ts) -> Int:
    return 2


def count[*Ts: Writable](a: Int, b: Int, c: Int, *rest: *Ts) -> Int:
    return 3


def mixed[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def mixed[*Ts: Writable](a: Int, b: String, c: Int, *rest: *Ts) -> Int:
    return 2


def copied[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def copied[*Ts: Writable](a: Int, var b: Int, c: Int, *rest: *Ts) -> Int:
    return 2


def owned[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def owned[*Ts: Writable](a: Int, var b: String, *rest: *Ts) -> Int:
    return 2


def referenced[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def referenced[*Ts: Writable](a: Int, ref b: Int, *rest: *Ts) -> Int:
    return 2


def referenced_by_value[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def referenced_by_value[*Ts: Writable](a: Int, ref b: Int, c: Int, *rest: *Ts) -> Int:
    return 2


def homogeneous(a: Int, *rest: Int) -> Int:
    return 1


def homogeneous(a: Int, b: Int, *rest: Int) -> Int:
    return 2


def literal(s: String) -> Int:
    return 1


def literal[*Ts: Writable](*args: *Ts) -> Int:
    return 2


def main():
    var x = 1
    var y = 2
    var s = String("s")
    print(count(x, x, x, x))
    print(count(x, x, x))
    print(count(x, x))
    print(mixed(x, s, x, x))
    print(mixed(x, "s", x, x))
    print(copied(x, x, x, x))
    print(owned(x, s, x))
    print(owned(x, s.copy(), x))
    print(owned(x, s^, x))
    print(referenced(x, y, x))
    print(referenced_by_value(x, y, x, x))
    print(homogeneous(x, x, x))
    print(literal("x"))
    print(literal(x))
