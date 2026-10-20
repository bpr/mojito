# A contiguous `List` slice is a `Span` borrowing the list's owned elements:
# a store through it shows in the list, it prints, compares, tests
# membership and truthiness like the list, passes to a `Span` parameter,
# iterates, and indexes, and an element reference through a temporary slice
# outlives the slice. A temporary view operand of an operator keeps its
# source alive through the comparison.
def total(s: Span[Int, _]) -> Int:
    var t = 0
    for x in s:
        t += x
    return t


def make() -> List[Int]:
    return [5, 6, 7]


def main():
    var xs: List[Int] = [1, 2, 3, 4]
    var s = xs[1:3]
    s[0] = 20
    print(xs)
    print(xs[1:3])
    var ys: List[Int] = [1, 20]
    print(xs[0:2] == xs[0:2], xs[0:2] == ys[0:2], xs[1:] != ys[0:1])
    print(3 in xs[1:], 1 in xs[1:])
    print(Bool(xs[3:]), Bool(xs[4:]))
    print(total(xs[1:]))
    for x in xs[:2]:
        print(x)
    print(xs[1:3][1])
    var w = xs[1:3]
    ref r = w[1]
    print(r)
    print(len(make()[1:]))
    var names: List[String] = ["a", "bc", "def"]
    print(names[1:], len(names[:0]))
    print(names[1:][0], names[1:][1].byte_length())
    var text = String("  ab  ")
    print(text.rstrip() == "  ab")
