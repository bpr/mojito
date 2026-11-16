# A generic function's own origin binder binds from its arguments' types
# alongside its type parameters: `first(Span(l))` solves `T` and `o`
# together, by value and as a `ref[o]` result, and over a pointer argument.
def first[T: ImplicitlyCopyable & Movable, o: Origin](s: Span[T, o]) -> T:
    return s[0]


def first_ref[
    T: ImplicitlyCopyable & Movable, o: Origin
](s: Span[T, o]) -> ref[o] T:
    return s[0]


def read[T: ImplicitlyCopyable & Movable, o: ImmOrigin](r: Pointer[T, o]) -> ref[o] T:
    return r[]


def main():
    var l = List[Int]()
    l.append(7)
    l.append(8)
    print(first(Span(l)))
    print(first_ref(Span(l)))
    var x = 222
    print(read(Pointer(to=x)))
