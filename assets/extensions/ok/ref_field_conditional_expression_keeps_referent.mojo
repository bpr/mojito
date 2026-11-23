# A conditional expression, or a value-returning `and`/`or`, building a
# struct with a `ref` field keeps the referent's owner live while the result
# is: the result's loans are the union of its arms', as the bare
# construction's are.
struct Holder[mut: Bool, //, o: Origin[mut=mut]]:
    var r: ref[Self.o] Int

    def __init__(out self, ref[Self.o] x: Int):
        self.r = x


struct H[mut: Bool, //, o: Origin[mut=mut]]:
    var r: ref[Self.o] String

    def __init__(out self, ref[Self.o] s: String):
        self.r = s

    def __bool__(self) -> Bool:
        return self.r.byte_length() > 0


def pick(c: Bool, d: Bool):
    var s = String("hello")
    var h = H(s) if c else (H(s) if d else H(s))
    print(h.r.byte_length())
    print((H(s) if c else (H(s) if d else H(s))).r.byte_length())


def pick_param[c: Bool]():
    var s = String("hello")
    var h = H(s) if c else H(s)
    print(h.r.byte_length())


def main():
    var x = 41
    var c = True
    var holder = Holder(x) if c else Holder(x)
    print(holder.r + 1)

    var s = String("hello")
    var h = H(s) if c else H(s)
    print(h.r.byte_length())
    print((H(s) if c else H(s)).r.byte_length())

    pick(True, False)
    pick(False, True)
    pick(False, False)
    pick_param[True]()
    pick_param[False]()
    for i in range(2):
        var k = H(s) if i == 0 else H(s)
        print(k.r.byte_length() + i)

    var empty = String("")
    var abc = String("abc")
    var g = H(empty) or H(empty)
    var j = H(empty) and H(empty)
    var m = H(abc) or H(abc)
    var n = H(abc) and H(abc)
    print(g.r.byte_length(), j.r.byte_length(), m.r.byte_length(), n.r.byte_length())
