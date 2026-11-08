# Pinned against Mojo 1.3.0.dev2026110105 (2026-11-01): a field read out of
# an expression temporary is a projection of the temporary's storage, so
# the temporary dies where the field's last use is. A register-passable
# `read` parameter loads the field at call setup, so the temporary dies
# before the call; `print`'s variadic pack and a generic `read` parameter
# lend the field, so it dies after. A call result, a conditional
# expression, and a chain through a nested field read the same way.
# Expected: del 5 / show 5 / 5 / del 5 / 3 / del 3 / gen 6 / del 6 /
# del 7 / show 7 / 8 / del 8 / del 9 / y 9 / wdel 10 / del 10 / show 10 /
# del 11 / 12 / del 12 / show 13
@fieldwise_init
struct R(ImplicitlyCopyable, RegisterPassable):
    var x: Int

    def __deinit__(deinit self):
        print("del", self.x)


@fieldwise_init
struct W(ImplicitlyCopyable, RegisterPassable):
    var r: R

    def __deinit__(deinit self):
        print("wdel", self.r.x)


def show(i: Int):
    print("show", i)


def gen[T: Writable](t: T):
    print("gen", t)


def make(n: Int) -> R:
    return R(n)


def main():
    show(R(5).x)
    print(R(5).x)
    var c = True
    print((R(3) if c else R(4)).x)
    gen(R(6).x)
    show(make(7).x)
    print(make(8).x)
    var y = R(9).x
    print("y", y)
    show(W(R(10)).r.x)
    print(R(11).x + 1)
    show(R(12).x + 1)
