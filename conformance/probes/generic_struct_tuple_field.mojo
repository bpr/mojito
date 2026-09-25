# PROBE (divergence): a generic struct's field of a `Tuple` built over its
# parameter.
#
# The pinned Mojo stores the tuple and reads its element. Mojito accepts the
# program and then stops at run time with "cannot index
# Tuple$t2[y3:Inty3:Int]"; a non-generic struct's `Tuple[Int, Int]` field
# runs.
#
# Observed 2026-09-25 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   2
#   mojito: run error
#
# When fixed: promote to `assets/ok`.
struct Holder[T: ImplicitlyCopyable & Deinitable](Movable):
    var pair: Tuple[Self.T, Int]

    def __init__(out self, var value: Self.T, count: Int):
        self.pair = Tuple[Self.T, Int](value, count)


def main():
    var some_int = Holder[Int](1, 2)
    print(some_int.pair[1])
