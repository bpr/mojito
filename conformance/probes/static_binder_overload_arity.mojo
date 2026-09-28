# PROBE: a generic struct's overloaded static, one member with a binder of
# its own (`pick[U: Writable](u: U)`) beside a two-argument member
# (`pick(u: Int, v: Int)`), called on a spelled receiver from a generic
# method: `Pair[Self.T].pick(1, 2)`.
#
# **Differs.** The pin runs it and prints `3` twice. Mojito's instance check
# retargets the two-argument call to the one-argument per-call clone and
# stops with "'Pair.pick$y3:Int$y3:Int' expects 1 argument(s), got 2".
# Filed in `docs/roadmap.md` §3. When Mojito prints `3` twice, promote this
# file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run static_binder_overload_arity.mojo


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def pick[U: Writable](u: U) -> Int:
        return 1

    @staticmethod
    def pick(u: Int, v: Int) -> Int:
        return 2


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def picked(self) -> Int:
        return Pair[Self.T].pick(1) + Pair[Self.T].pick(1, 2)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.picked())
    print(words.picked())
