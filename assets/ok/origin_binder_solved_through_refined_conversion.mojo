# An origin binder a parameter names solves through an implicit conversion
# whose initializer refines `out self`: a literal passed where a
# `StringSpan[o]` is declared binds `o` to the static origin of
# `StringSpan.__init__(out self: StaticString, lit: StringLiteral)`, as the
# converted value's type does — in a free call, a method's own binder, a
# struct's binder a construction infers, two arguments agreeing on one
# binder, and a user struct whose refinement also closes its `mut=` binder.
struct Holder[o: ImmOrigin]:
    var s: StringSpan[Self.o]

    def __init__(out self, s: StringSpan[Self.o]):
        self.s = s

    def total[p: ImmOrigin](self, t: StringSpan[p]) -> Int:
        return self.s.byte_length() + t.byte_length()


struct V[mut: Bool, //, o: Origin[mut=mut]](ImplicitlyCopyable):
    var x: Int

    @implicit
    def __init__(out self: V[ImmStaticOrigin], x: Int):
        self.x = x


def length[o: ImmOrigin](s: StringSpan[o]) -> Int:
    return s.byte_length()


def echo[o: ImmOrigin](s: StringSpan[o]) -> StringSpan[o]:
    return s


def both[o: ImmOrigin](a: StringSpan[o], b: StringSpan[o]) -> Int:
    return a.byte_length() + b.byte_length()


def view_x[o: ImmOrigin](v: V[o]) -> Int:
    return v.x


def view_mut[m: Bool, //, o: Origin[mut=m]](v: V[o]) -> Bool:
    return m


def main():
    var a = String("abc")
    print(length("xy"), length(a))
    print(echo("hello"))
    var h = Holder("xyz")
    print(h.s, h.total("ab"))
    print(both("ab", "cde"))
    comptime lit = "qq"
    print(echo(lit))
    print(view_x(5), view_mut(7))
