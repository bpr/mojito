# The public `Tuple` is an ordinary struct generator: one template in
# `std/builtin/tuple.mojo` serves every element list. A temporary tuple is
# indexed and unpacked through the reference its accessor returns, the
# consuming `reverse` and `concat` move each element out through a pointer,
# and the empty tuple is an instance like any other.
def make() -> Tuple[Int, String]:
    return (1, "a")


def flip(t: Tuple[Int, String]) -> Tuple[String, Int]:
    return t.reverse()


def main():
    var d, e = make()
    print(d, e)
    var x = 3
    var y = 4
    print(x, y)
    x, y = (5, 6)
    print(x, y)
    print(make()[1])
    var t = (String("abc"), 2, 7)
    var r = t^.reverse()
    print(r[0], r[1], r[2])
    var u = (7, 2, String("abc"))
    var c = u^.concat((8, 3.5))
    print(c[2], c[3], c[4], len(c))
    var empty = ()
    print(len(empty), empty)
    var zero = Tuple[Int, Bool, Float64]()
    print(zero[0], zero[1], zero[2])
    print(zero == Tuple[Int, Bool, Float64](), 2 in (1, 2, 3))
    var kept: Tuple[Int, String] = (1, "x")
    var flipped = flip(kept)
    print(flipped[0], flipped[1])
    var joined = kept.concat(Tuple(True))
    print(joined[2], kept[1])
