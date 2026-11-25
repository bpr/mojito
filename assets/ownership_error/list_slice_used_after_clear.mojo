# expect: use of invalidated interior reference 'ys' to 'xs["element"]'
# A contiguous `List` slice is a `Span` over
# `origin_of(xs)._get_owned_interior["element"]`, so clearing the list
# stales the slice, and its next use is rejected as the pin rejects it.
# requires: stdlib
def main():
    var xs: List[Int] = [1, 2, 3]
    var ys = xs[1:3]
    xs.clear()
    print(len(ys))
