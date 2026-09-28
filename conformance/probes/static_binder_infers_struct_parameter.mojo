# PROBE: a generic struct's static with a binder of its own, called from a
# generic method both on a spelled receiver, `Pair[Self.T].both(1,
# self.item)`, and on the bare struct name, `Pair.both(7, self.item)`, whose
# struct parameter is then inferred from an argument of the caller's
# parameter type.
#
# **Differs.** The pin runs it and prints `1 3`, `1 x`, `3 3`, `7 3`, `7 x`,
# `3 3`. Mojito rejects it with "cannot infer type parameter 'T' of 'Pair'
# from the arguments"; either call alone runs. Filed in `docs/roadmap.md`
# §3. When Mojito runs it, promote this file to `assets/ok/` with its
# manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run static_binder_infers_struct_parameter.mojo


@fieldwise_init
struct Pair[T: Copyable & Deinitable & Writable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def both[U: Writable](u: U, t: Self.T) -> Int:
        print(u, t)
        return 3


struct Shelf[T: Copyable & Deinitable & Writable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def spelled(self) -> Int:
        return Pair[Self.T].both(1, self.item)

    def inferred(self) -> Int:
        return Pair.both(7, self.item)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.spelled(), words.spelled())
    print(ints.inferred(), words.inferred())
