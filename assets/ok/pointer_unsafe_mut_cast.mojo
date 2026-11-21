# `Pointer.unsafe_mut_cast[m]()` returns the pointer at the origin
# `origin.unsafe_mut_cast[m]()`: a cast to `True` is mutable even over the
# immutable static origin or an immutable borrow, and a cast to `False`
# reads back through the immutable pointer.
def show(p: MutPointer[Byte, _]):
    print(p[])


def poke(p: MutPointer[Int, _]):
    p[] = 7


def bump(xs: List[Int]):
    poke(xs.unsafe_ptr().unsafe_mut_cast[True]())


def read_back[o: MutOrigin](p: Pointer[Int, o]) -> Int:
    var q = p.unsafe_mut_cast[False]()
    return q[]


def main():
    show("a".ptr().unsafe_mut_cast[True]())
    var xs: List[Int] = [1, 2]
    bump(xs)
    print(xs[0])
    print(read_back(xs.unsafe_ptr()))
    var u = "b".ptr().unsafe_mut_cast[True]().unsafe_mut_cast[False]()
    print(u[])
