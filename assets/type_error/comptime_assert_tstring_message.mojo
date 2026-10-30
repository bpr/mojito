# expect: function instantiation of `f` failed: constraint failed: n is 3 (want > 5)
# A `comptime assert` message may be a t-string over the instance's
# parameters: the pin writes it as the `String` it formats, evaluated for the
# instance the assertion fails.

def f[n: Int]():
    comptime assert n > 5, t"n is {n} (want > 5)"
    print(n)


def main():
    f[7]()
    f[3]()
