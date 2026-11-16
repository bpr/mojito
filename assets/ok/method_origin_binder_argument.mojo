# A method's own origin binder binds from its arguments' types, on a
# temporary receiver, a generic method, a `String` pointee, and a static
# method: each call selects the method and its `ref[o]` result keeps the
# pointee alive.
struct S:
    def __init__(out self):
        pass

    def read[o: ImmOrigin](self, r: Pointer[Int, o]) -> ref[o] Int:
        return r[]

    def first[T: ImplicitlyCopyable & Movable, o: Origin](self, s: Span[T, o]) -> T:
        return s[0]

    def read_string[o: ImmOrigin](self, r: Pointer[String, o]) -> ref[o] String:
        return r[]

    @staticmethod
    def read_static[o: ImmOrigin](r: Pointer[Int, o]) -> ref[o] Int:
        return r[]


def main():
    var x = 222
    print(S().read(Pointer(to=x)))
    var l = List[Int]()
    l.append(7)
    l.append(8)
    print(S().first(Span(l)))
    var s = String("hello!")
    print(S().read_string(Pointer(to=s)))
    var y = 223
    print(S.read_static(Pointer(to=y)))
