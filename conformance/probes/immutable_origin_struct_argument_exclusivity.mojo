# PROBE: a value whose type names an immutable origin, handed to a method of a receiver naming the same origin.
#
# **Differs.** The pin prints "1 2". Mojito rejects the call: "aliasing
# values passed mutably to 'self' argument and passed mutably to 'value'
# argument in 'push' call". The argument exclusivity check counts the origin
# of `Span[Int, ImmOrigin(origin_of(xs))]` as mutable, where the same shape
# over a `Pointer` is accepted. Filed in `docs/roadmap.md` §3. When Mojito
# runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run immutable_origin_struct_argument_exclusivity.mojo
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)


def main():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Span[Int, ImmOrigin(origin_of(xs))]]()
    var s: Span[Int, ImmOrigin(origin_of(xs))] = Span(xs)
    b.push(s)
    print(len(b.items), b.items[0][1])
