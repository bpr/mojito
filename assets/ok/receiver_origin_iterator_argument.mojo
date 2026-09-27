# An iterator a method returns over a loan-carrying list
# (`l.__iter__()` over `List[Span[Int, origin_of(xs)]]`) is recorded with the
# method's receiver origin in its origin slot. Handed to `next` or to a user
# generic `def`, that slot becomes a clone binder as a caller's place does,
# so each call gets its own clone rather than the erased body. Natively the
# iterator's instance, reached with the list's place and its binder, is one
# instance: origins inside a `ref` field's referent erase.
from std.iter import Iterator


def advance[I: Iterator](mut it: I) raises -> I.Element:
    return next(it)


def main() raises:
    var xs: List[Int] = [4, 5, 6]
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    l.append(Span(xs))
    var it = l.__iter__()
    var s = next(it)
    print(len(s), s[0])
    var t = advance(it)
    print(t[2])
    print(xs[1])
