# A nested `def` that captures nothing is a compile-time value: a local
# `comptime`, a parameter argument, or a `comptime if` condition calls it,
# and its run-time calls run beside them.


def first[k: Int]():
    print("first", k)


def main():
    def mk() -> Int:
        return 7

    comptime t = mk()
    print(t)
    first[mk()]()

    def add(a: Int, b: Int) -> Int:
        return a + b

    comptime s = add(5, 4)
    var r = add(6, 4)
    print(s, r)

    def positive() -> Bool:
        return mk() > 0

    comptime if positive():
        print("positive")
    else:
        print("not positive")
