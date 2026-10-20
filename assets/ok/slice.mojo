# Slice subscripts `a[i:j:k]` on List. Contiguous List slices are strict
# (see list_contiguous_slice_strict.mojo and the runtime_error fixtures);
# strided List slicing keeps Python semantics (negative indices, optional
# bounds, negative step reverses). String-family positional slicing —
# including StringLiteral values — was removed at the audited head; the
# nominal String's keyword slicing is pinned by the nominal_string_* fixtures.
# A contiguous slice is a `Span` borrowing the list's elements; a strided
# slice is a copied `List`.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    var mid = xs[1:3]
    print(mid[0], mid[1], len(mid))
    print(xs[::-1])
    print(xs[-2::1])
