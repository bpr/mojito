# A `comptime for` over a module list of struct values in a generic `def`
# runs its body once per value, as the same loop in a plain `def` does. The
# elements are built by compile-time construction, whose evaluation sees only
# the declarations its entry reaches: a callable parameter named like the
# generic `def` (`f`) does not pull that `def` in. Output matches the pin.


@fieldwise_init
struct P(Copyable, ImplicitlyCopyable, Movable):
    var a: Int
    var b: Int


comptime PS = [P(1, 2), P(3, 4)]


def f[n: Int]():
    comptime for p in PS:
        print(p.a + n, p.b)


def main():
    f[10]()
