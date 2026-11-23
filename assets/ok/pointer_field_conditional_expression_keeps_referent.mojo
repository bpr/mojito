# A conditional expression building a pointer-field struct keeps the
# pointee's owner live while the result is, as the bare construction does.
struct H[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[String, Self.o]

    def __init__(out self, ref[Self.o] s: String):
        self.p = Pointer(to=s)


struct Tagged[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[String, Self.o]
    var tag: String

    def __init__(out self, ref[Self.o] s: String):
        self.p = Pointer(to=s)
        self.tag = String("t")


def pick[c: Bool]():
    var s = String("hello")
    var h = H(s) if c else H(s)
    print(h.p[].byte_length())


def main():
    var s = String("hello")
    var c = True
    var h = H(s) if c else H(s)
    print(h.p[].byte_length())
    print((H(s) if c else H(s)).p[].byte_length())
    var t = Tagged(s) if c else Tagged(s)
    print(t.p[].byte_length(), t.tag)
    var q = Pointer(to=s) if c else Pointer(to=s)
    print(q[].byte_length())
    pick[True]()
    pick[False]()
