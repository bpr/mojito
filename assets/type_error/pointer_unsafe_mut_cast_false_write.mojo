# expect: write through a Pointer whose origin is immutable
# `Pointer.unsafe_mut_cast[False]()` over a mutable borrow returns an
# immutable pointer, so a write through it is rejected.
def main():
    var xs: List[Int] = [1, 2]
    var q = xs.unsafe_ptr().unsafe_mut_cast[False]()
    q[] = 5
    print(xs[0])
