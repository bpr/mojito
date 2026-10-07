# A tuple or struct value parameter holding a `String` converts a literal
# argument element to `String`, and each instance builds the value it reads.
@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int
    var s: String


def show[p: Tuple[Int, String]]():
    print(p[0], p[1])


def whole[p: Tuple[Int, String]]():
    var t = p
    print(t[1])
    print(p)


def mutate[p: Tuple[Int, String]]():
    var s: String = p[1]
    s += "b"
    print(p[0], s, p[1].byte_length())


def nested[p: Tuple[String, Tuple[Int, String]]]():
    print(p[0], p[1][0], p[1][1])


def defaulted[p: Tuple[Int, String] = (7, "d")]():
    print(p[0], p[1])


def fields[q: Q]():
    print(q.a, q.s)
    var r = q
    var s = r.s
    s += "y"
    print(r.a, s)


def main():
    show[(1, "a")]()
    whole[(2, "b")]()
    mutate[(3, "c")]()
    mutate[(4, "c")]()
    nested[("x", (5, "e"))]()
    defaulted()
    defaulted[p=(6, "k")]()
    fields[Q(8, "x")]()
