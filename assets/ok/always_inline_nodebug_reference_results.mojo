# A call of an `@always_inline("nodebug")` function or method returning a
# reference holds the callee's body, the reference it returns made to the
# caller's place the call passes: through a compound assignment, a read, a
# caller's own `mut` parameter, a field, a borrowed argument, a callee of
# several returns, and a parametric struct's member.


@always_inline("nodebug")
def pick(ref x: Int) -> ref [x] Int:
    return x


@always_inline("nodebug")
def either(c: Bool, ref a: Int, ref b: Int) -> ref [a, b] Int:
    if c:
        return a
    return b


@always_inline("nodebug")
def view(s: String, n: Int) -> ref [s] String:
    return s


@always_inline("nodebug")
def name_of(ref s: String) -> ref [s] String:
    return s


@always_inline("nodebug")
def through(ref x: Int) -> ref [x] Int:
    return pick(x)


struct Box[n: Int]:
    var v: Int
    var label: String

    def __init__(out self, v: Int, var label: String):
        self.v = v
        self.label = label^

    @always_inline("nodebug")
    def val(ref self) -> ref [self.v] Int:
        return self.v

    @always_inline("nodebug")
    def lab(ref self) -> ref [self.label] String:
        return self.label


struct Pair:
    var a: Int
    var b: Int

    def __init__(out self, a: Int, b: Int):
        self.a = a
        self.b = b


def bump(mut z: Int):
    pick(z) += 100


def main():
    var y = 1
    var w = 5
    pick(y) += 1
    either(True, y, w) += 10
    either(False, y, w) += 20
    print(y, w)
    through(y) += 3
    print(y)
    bump(y)
    print(y)
    var p = Pair(1, 2)
    pick(p.b) += 7
    print(p.a, p.b)
    var s = String("hi")
    name_of(s) += " there"
    print(name_of(s))
    var b = Box[3](4, "lbl")
    b.val() += 1
    b.lab() += "!"
    print(b.val(), b.lab())
    var t = name_of(s)
    print(t)
    print(view(t, 2))
