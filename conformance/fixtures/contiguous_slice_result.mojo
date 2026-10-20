# A contiguous `List` slice is a `Span` borrowing the list's owned elements,
# so both compilers reject returning it as an owned `List[Int]`.
def mid(xs: List[Int]) -> List[Int]:
    return xs[1:3]

def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(mid(xs))
