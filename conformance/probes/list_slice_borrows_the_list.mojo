# PROBE: a contiguous `List` slice used after the list is cleared.
#
# The pin rejects it: "use of invalidated interior reference
# 'xs["element"]'", since `xs[1:3]` is a `Span` borrowing the list. Mojito's
# bundled `List.__getitem__(ContiguousSlice)` returns a copied `List`, so it
# accepts the program and prints 2.
#
# Observed 2026-10-13 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run list_slice_borrows_the_list.mojo
#         cargo run -- run conformance/probes/list_slice_borrows_the_list.mojo
def main():
    var xs: List[Int] = [1, 2, 3]
    var ys = xs[1:3]
    xs.clear()
    print(len(ys))
