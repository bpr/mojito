# A value read of a declared `ref` field off an expression temporary reads
# through the stored handle, exactly as the same read off a named local
# does: the call result's slot projects the field, and the value is the
# referent. A memory-only referent's copy passes the stored handle as its
# copy initializer's source.
struct Holder[o: Origin[mut=False]]:
    var r: ref[Self.o] Int

    def __init__(out self, ref[Self.o] x: Int):
        self.r = x

struct Named[o: Origin[mut=False]]:
    var r: ref[Self.o] String

    def __init__(out self, ref[Self.o] x: String):
        self.r = x

def make[o: Origin[mut=False]](ref[o] x: Int) -> Holder[o]:
    return Holder(x)

def main():
    var x = 3
    var n = Holder(x).r
    print(n + 1)
    print(Holder(x).r + 1)
    var m: Int = make(x).r
    print(m * 2)
    var s = String("hi")
    var t = Named(s).r
    print(t)
    print(Named(s).r.byte_length())
    var h = Named(s)
    var u = h.r
    print(u + "!")
