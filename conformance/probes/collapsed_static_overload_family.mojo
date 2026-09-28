# PROBE: a generic struct's overloaded static whose members collapse at an
# instance: `pick(v: Self.T)` beside `pick(v: Float64)`, called at
# `T = Float64`.
#
# **Differs.** The pin runs it and prints `1`, the member declared over the
# struct's parameter, in either declaration order. Mojito prints `2`, the
# member declared `Float64`. Filed in `docs/roadmap.md` §3. When Mojito
# prints `1`, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run collapsed_static_overload_family.mojo


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def pick(v: Self.T) -> Int:
        return 1

    @staticmethod
    def pick(v: Float64) -> Int:
        return 2


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def picked(self) -> Int:
        return Pair[Self.T].pick(self.item)


def main():
    print(Shelf[Float64](2.5).picked())
