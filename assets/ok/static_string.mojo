# `StaticString` is upstream's `StringSpan[ImmStaticOrigin]`: a literal
# converts to it wherever one is expected, as a parameter, a field, a
# return, a variable, or a collection element.
struct Named(Copyable):
    var name: StaticString

    def __init__(out self, name: StaticString):
        self.name = name


def pick(b: Bool) -> StaticString:
    return StaticString("True") if b else StaticString("False")


def main():
    print(pick(True), pick(False))
    var n = Named("field")
    print(n.name, n.name.byte_length())
    var s: StaticString = "abc"
    print(s == "abc", s.startswith("a"), String(s) + "!")
    var l = List[StaticString]()
    l.append("x")
    l.append("yz")
    for e in l:
        print(e)
