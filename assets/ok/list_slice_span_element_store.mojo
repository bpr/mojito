# An element store while a contiguous `List` slice is live. The slice is a
# `Span` over `origin_of(xs)._get_owned_interior["element"]`, which an
# element store does not move, so the slice reads the stored element, as at
# the pin: `[9, 1, 2, 3, 4]` (R1160).
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    var v = xs[:]
    xs[0] = 9
    print(v)
