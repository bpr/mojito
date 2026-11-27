# A default solves the binders its parameter's type names when a call
# leaves the argument out, as an argument of the default's type would: a
# static `StaticString` or a literal binds an origin binder, an `@implicit`
# conversion binds an origin and its `mut=` binder together, and an
# evaluated default runs at the type it solved. The same holds of a
# method's own binders, through a trait bound, in a method called before
# its struct is declared, and in a compile-time application.
def label[o: ImmOrigin](s: StringSpan[o] = StaticString("d")) -> StringSpan[o]:
    return s


def size[o: ImmOrigin](s: StringSlice[o] = "x") -> Int:
    return s.byte_length()


def caller() -> Int:
    var t = Tagger()
    return t.tag()


struct V[mut: Bool, //, o: Origin[mut=mut]](ImplicitlyCopyable):
    var x: Int

    def __init__(out self, *, other: Int):
        self.x = other

    @implicit
    def __init__(out self: V[ImmStaticOrigin], x: Int):
        self.x = x


def read[m: Bool, //, o: Origin[mut=m]](v: V[o] = 7) -> Int:
    return v.x


def built[m: Bool, //, o: Origin[mut=m]](
    v: V[o] = V[ImmStaticOrigin](other=9),
) -> Int:
    comptime if m:
        return v.x + 100
    return v.x


trait Greets:
    def greet[o: ImmOrigin](self, s: StringSpan[o] = StaticString("hi")) -> Int:
        ...


struct Tagger(Greets):
    def __init__(out self):
        pass

    def tag[o: ImmOrigin](self, s: StringSpan[o] = StaticString("tag")) -> Int:
        return s.byte_length()

    def greet[o: ImmOrigin](self, s: StringSpan[o] = StaticString("hi")) -> Int:
        return s.byte_length() + 10


def through[T: Greets](t: T) -> Int:
    return t.greet()


def main():
    var a = String("abcd")
    print(label(), label(StaticString("x")), label(a))
    print(size(), size("abc"), size(a))
    print(read(), read(V[MutAnyOrigin](other=3)), read(5))
    print(built(), built(V[MutAnyOrigin](other=3)), built(5))
    var t = Tagger()
    print(caller(), t.tag(StaticString("ab")), through(t), t.greet("abc"))
    comptime n = size()
    print(n)
