# A place handed to a `var` parameter must be copied, and every overload
# candidate is charged that copy. The copy outranks the signature-length
# tie-break that prefers a concrete candidate, so `h(s)` selects the generic
# overload while the rvalue `h(String("t"))` selects the `var` one. A keyword
# slot and a field place cost the copy a positional place costs, and one copy
# still ranks below one conversion.
# requires: discovery


@fieldwise_init
struct Holder(Copyable, Movable):
    var s: String


def h(var a: String) -> Int:
    return 1


def h[T: Writable](a: T) -> Int:
    return 2


def pair(var a: String, b: Int) -> Int:
    return 1


def pair(a: String, b: Float64) -> Int:
    return 2


def main():
    var s = String("s")
    var box = Holder(String("b"))
    print(h(s))
    print(h(String("t")))
    print(h(a=s))
    print(h(box.s))
    print(pair(s, 1))
