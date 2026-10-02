# PROBE: a pointer handed to a struct's constructor, read through the struct after the pointee's last direct use.
#
# **Differs.** The pin prints 2, 2, 2 and 3. Mojito stops with "use after
# Pointer deallocation": the cell built from `p` takes no loan on `xs`, so
# `xs` is destroyed after the pointer's own last use. The last line also
# stops alone, with "methods on ref", which is its own entry. Filed in
# `docs/roadmap.md` §3. When Mojito runs it, promote this file to
# `assets/ok/` with its manifest rows.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run pointer_copied_into_struct_keeps_loan.mojo
struct Cell[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var first: Self.T):
        self.item = first^

    def get(self) -> Self.T:
        return self.item.copy()


def main():
    var xs: List[Int] = [1, 2, 3]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    var c = Cell[Pointer[List[Int], ImmOrigin(origin_of(xs))]](p)
    print(c.item[][1])
    print(c.get()[][1])
    var q = c.get()
    print(q[][1])
    print(len(c.item[]))
