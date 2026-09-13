# A contiguous `List` slice is an owned `List` in Mojito and a borrowing
# `Span` upstream, so only Mojito returns one from a `-> List[Int]` function.
def mid(xs: List[Int]) -> List[Int]:
    return xs[1:3]

def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(mid(xs))
