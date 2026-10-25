# A condition over a `Boolable` value converts it as `Bool(x)` does, reading
# the value where it lies: a named struct, an `Optional`, a field, a list
# or tuple element, and a `T: Boolable` parameter are not copied, and a
# temporary is destroyed once its `__bool__` returns.
struct T(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("  copy", self.v)

    def __deinit__(deinit self):
        print("  del", self.v)


struct C(Boolable, Copyable):
    var t: T

    def __init__(out self, var t: T):
        self.t = t^

    def __bool__(self) -> Bool:
        return self.t.v != 0


struct H:
    var o: Optional[T]

    def __init__(out self, var o: Optional[T]):
        self.o = o^

    def check(self) -> Bool:
        if self.o:
            return True
        return False


def gen[B: Boolable & Copyable](b: B) -> Bool:
    if b:
        return True
    return False


def make() -> C:
    return C(T(5))


def main():
    var o = Optional(T(8))
    print("field")
    var h = H(Optional(T(3)))
    print(h.check())
    print("generic")
    var c = C(T(9))
    print(gen(c))
    print("temporary")
    if make():
        print("made")
    print("while")
    var n = 0
    while o:
        n += 1
        if n >= 2:
            break
    while c:
        break
    print("ternary")
    var s = "y" if o else "n"
    print(s)
    print("not")
    if not o:
        print("none")
    print("comprehension")
    var xs = [o.copy(), o.copy()]
    var ys = [1 for x in xs if x]
    print(len(ys))
    print("elements")
    var os = List[Optional[T]]()
    os.append(Optional(T(1)))
    os.append(None)
    for i in range(len(os)):
        if os[i]:
            print("set", i)
    var pair = (C(T(2)), Optional(T(4)))
    if pair[0]:
        if pair[1]:
            print("pair")
    os.append(Optional(T(7)))
    print(len(os))
    print("end", o.value().v, c.t.v, h.o.value().v)
