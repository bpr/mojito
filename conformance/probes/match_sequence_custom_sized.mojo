# PROBE: a sequence pattern over a user `Sized` type with `__getitem__(Int)`: one `__len__` call per case gates the element reads, `_` reads nothing, a from-end element calls `__len__` again.
#
# The pin runs it and prints:
#   len
#   len
#   get 0
#   get 2
#   three 12
#   len
#   len
#   get 2
#   last 12
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects sequence patterns until R1135 lands.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run match_sequence_custom_sized.mojo
#         cargo run -- run conformance/probes/match_sequence_custom_sized.mojo
struct Trio(Sized, ImplicitlyCopyable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __len__(self) -> Int:
        print("len")
        return 3

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return self.base + i


def main():
    var t = Trio(10)
    match t:
        case [a, b]:
            print("two", a, b)
        case [10, _, c]:
            print("three", c)
        case _:
            print("other")
    match t:
        case [_, *_, 12]:
            print("last 12")
        case _:
            print("other")
