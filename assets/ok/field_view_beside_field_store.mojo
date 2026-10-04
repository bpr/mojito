# A view of one field's owned interior coexists with reads of that field and
# with stores to a sibling field, and a store over the field is fine once the
# view is dead — through `self`, a `mut` parameter, or a local.
struct Named(Movable):
    var name: String
    var other: String

    def __init__(out self, var name: String):
        self.name = name^
        self.other = String("o")

    def dead_view(mut self) -> Int:
        var view = self.name.strip()
        var n = view.byte_length()
        self.name = String("q")
        return n + self.name.byte_length()

    def sibling(mut self) -> Int:
        var view = self.name.strip()
        self.other = String("zz")
        var n = self.name.byte_length()
        return view.byte_length() + n + self.other.byte_length()


def bump(mut h: Named) -> Int:
    var view = h.name.strip()
    h.other = String("k")
    var n = view.byte_length()
    h.name = String("abc")
    return n + h.name.byte_length()


def main():
    var a = Named(String("  ab  "))
    print(a.dead_view())
    var b = Named(String(" xy "))
    print(b.sibling())
    print(bump(b))
    var view = b.name.strip()
    b.other = String("m")
    print(view.byte_length(), b.other)
