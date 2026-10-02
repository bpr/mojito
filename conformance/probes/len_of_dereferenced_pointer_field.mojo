# PROBE: `len` of a list reached through a pointer held in a struct field.
#
# **Differs.** The pin prints 3. Mojito stops with "vm backend does not
# support methods on ref yet". Filed in `docs/roadmap.md` §3. When Mojito
# runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run len_of_dereferenced_pointer_field.mojo
struct Cell[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var first: Self.T):
        self.item = first^

    def put(mut self, var value: Self.T):
        self.item = value^


def main():
    var xs: List[Int] = [1, 2, 3]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    var c = Cell[Pointer[List[Int], ImmOrigin(origin_of(xs))]](p)
    c.put(p)
    print(len(c.item[]))
