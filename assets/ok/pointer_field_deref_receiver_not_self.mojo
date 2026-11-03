# A pointer field's pointee is not a place under the struct holding the
# pointer: a view of another field may be passed to a mutating method
# reached through it.
struct Sink(Writer):
    var n: Int

    def __init__(out self):
        self.n = 0

    def write_string(mut self, string: StringSlice):
        self.n += string.byte_length()


struct B[o: MutOrigin]:
    var p: Pointer[Sink, Self.o]
    var s: String

    def __init__(out self, ref[Self.o] sink: Sink):
        self.p = Pointer(to=sink)
        self.s = "abc"

    def f(mut self):
        self.p[].write_string(StringSlice(self.s))


def main():
    var k = Sink()
    var b = B(k)
    b.f()
    print(k.n)
