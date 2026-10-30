# `String(a, b, …)` writes each `Writable` argument through upstream's
# `__init__[*Ts: Writable](out self, *args: *Ts, sep=, end=)`, while one
# positional argument stays the stringify conversion; a `comptime assert`
# message may build one over its parameters.
def check[n: Int]() -> Int:
    comptime assert n < 10, String("n is ", n)
    return n


def main():
    print(String("span:", 3, "..", 7))
    var n = 3
    print(String("n is ", n))
    print(String(1, 2.5, "three", sep=", "))
    print(String("a", "b", end="!"))
    print(String(4), String("x"), check[2]())
