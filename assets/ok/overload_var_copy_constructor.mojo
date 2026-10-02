# A constructor overload set ranks as a free function's does. The implicit
# copy a place costs `__init__(out self, var a: String)` outranks the
# specificity tie-break, so `Tag(s)` selects the generic constructor while the
# rvalue `Tag(String("t"))` selects the `var` one. A generic constructor runs
# as a per-call clone of its own, and two generic constructors baked alike are
# told apart by the call's arguments.
# requires: discovery


struct Tag:
    var v: Int
    var text: String

    def __init__(out self, var a: String):
        self.v = 1
        self.text = a

    def __init__[T: Writable](out self, a: T):
        self.v = 2
        self.text = String(a)

    def __init__[T: Writable](out self, a: T, b: Int):
        self.v = 3 + b
        self.text = String(a)


struct Converted:
    var v: Int

    def __init__(out self, a: Float64):
        self.v = 1

    def __init__[T: Writable](out self, a: T):
        self.v = 2


def main():
    var s = String("s")
    var a = Tag(s)
    print(a.v, a.text)
    var b = Tag(String("t"))
    print(b.v, b.text)
    var c = Tag(7)
    print(c.v, c.text)
    var d = Tag(2.5)
    print(d.v, d.text)
    var e = Tag(True, 4)
    print(e.v, e.text)
    var f = Tag(s, 1)
    print(f.v, f.text)
    print(s)
    var i: Int = 3
    print(Converted(i).v)
    print(Converted(2.5).v)
