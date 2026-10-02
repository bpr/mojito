# A generic constructor declared beside another on a generic struct runs as a
# per-call clone of the instance it constructs, keyed by the instance's
# arguments and then the call's. `Tag[Int](s)` selects the generic constructor
# as `Tag(s)` does on a non-generic struct, the rvalue selects the `var` one,
# and two generic constructors baked alike are told apart by the call.
# requires: discovery


struct Tag[U: Copyable & Writable & Deinitable]:
    var v: Int
    var text: String
    var extra: Self.U

    def __init__(out self, var a: String, extra: Self.U):
        self.v = 1
        self.text = a
        self.extra = extra.copy()

    def __init__[T: Writable](out self, a: T, extra: Self.U):
        self.v = 2
        self.text = String(a)
        self.extra = extra.copy()

    def __init__[T: Writable](out self, a: T, extra: Self.U, b: Int):
        self.v = 3 + b
        self.text = String(a)
        self.extra = extra.copy()


struct Only[U: AnyType]:
    var v: Int

    def __init__[T: Writable](out self, a: T):
        self.v = 5


def main():
    var s = String("s")
    var a = Tag[Int](s, 10)
    print(a.v, a.text, a.extra)
    var b = Tag[Int](String("t"), 11)
    print(b.v, b.text, b.extra)
    var c = Tag[Int](7, 12)
    print(c.v, c.text, c.extra)
    var d = Tag[String](2.5, s)
    print(d.v, d.text, d.extra)
    var e = Tag[Int](True, 13, 4)
    print(e.v, e.text, e.extra)
    var f = Tag[Bool](7, True)
    print(f.v, f.text, f.extra)
    var g = Tag(7, 2.5)
    print(g.v, g.text, g.extra)
    print(s)
    print(Only[Int](7).v, Only[String](7).v, Only[Int](s).v)
