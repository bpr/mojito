# expect: use of uninitialized value 'a'
# A by-value place argument is read at call setup, after every argument has
# been evaluated: a later argument that moves the place leaves the earlier
# read uninitialized, as the pinned Mojo rejects it.
@fieldwise_init
struct C(Copyable, Movable):
    var t: Int


def take(var w: C) -> Int:
    return 0


def two(a: Int, b: Int):
    print("two", a, b)


def main():
    var a = C(1)
    two(a.t, take(a^))
