# PROBE: a heap-owning temporary that carries a loan, handed to an owning parameter.
#
# **Differs.** The pin prints 1 and 1. Mojito stops with "use after Pointer
# deallocation": the temporary `Maybe(Span(xs))` is anchored in a hidden slot
# to keep `xs` alive through the call, the callee takes the same value as its
# own, and both destroy the list inside it. Filed in `docs/roadmap.md` §3.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest
# rows.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run loan_carrying_temporary_owned_argument.mojo
struct Maybe[T: Copyable & Deinitable](Copyable, Movable):
    var data: List[Self.T]

    def __init__(out self, var value: Self.T):
        self.data = List[Self.T]()
        self.data.append(value^)

    def count(self) -> Int:
        return len(self.data)


def take[T: Copyable & Deinitable](var m: Maybe[T]) -> Int:
    return m.count()


def run(xs: List[Int]):
    print(take(Maybe(Span(xs))))
    var l = List[Maybe[Span[Int, origin_of(xs)]]]()
    l.append(Maybe(Span(xs)))
    print(l[0].count())


def main():
    run([1, 2, 3])
