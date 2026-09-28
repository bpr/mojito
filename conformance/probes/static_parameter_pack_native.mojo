# PROBE: a generic struct's static taking a read-only pack of the struct's
# parameter type, `count(*values: Self.T)`, called on a spelled receiver:
# `Pair[Self.T].count(self.item, self.item)`.
#
# **Differs natively.** The pin and Mojito's VM print `2 2`. The native
# backend stops with "pliron backend: in `Pair.count`: unsupported
# unresolved type parameter `T`". Filed in `docs/roadmap.md` §2. When
# `run --backend pliron` prints `2 2`, promote this file to `assets/ok/`
# with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run static_parameter_pack_native.mojo


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def count(*values: Self.T) -> Int:
        return len(values)


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def counted(self) -> Int:
        return Pair[Self.T].count(self.item, self.item)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.counted(), words.counted())
