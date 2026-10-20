# expect: conflicts with live reference
# A contiguous `List` slice is a `Span` borrowing the list, so clearing the
# list while the slice is live conflicts with it. The pin words this as the
# interior invalidation `use of invalidated interior reference 'xs["element"]'`
# (R266 owns the wording).
# requires: stdlib
def main():
    var xs: List[Int] = [1, 2, 3]
    var ys = xs[1:3]
    xs.clear()
    print(len(ys))
