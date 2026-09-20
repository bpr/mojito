# Among variadic candidates, a collector that takes at least one argument
# outranks both the implicit copy a place costs a `var` parameter and the
# signature-length tie-break that prefers fewer compile-time parameters. So
# `copied(s, 1)` keeps the candidate that copies `s`, because the other's
# collector would be empty, and `wide(x, x, x)` takes the candidate with one
# more compile-time parameter for the same reason.
# requires: discovery


def copied[*Ts: Writable](var a: String, *rest: *Ts) -> Int:
    return 1


def copied[*Ts: Writable](a: String, b: Int, *rest: *Ts) -> Int:
    return 2


def wide[*Ts: Writable](a: Int, b: Int, c: Int, *rest: *Ts) -> Int:
    return 3


def wide[T: Writable, *Ts: Writable](a: Int, b: T, *rest: *Ts) -> Int:
    return 4


def main():
    var s = String("s")
    var x = 1
    print(copied(s, 1))
    print(wide(x, x, x))
