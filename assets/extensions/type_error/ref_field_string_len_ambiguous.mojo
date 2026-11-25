# expect: `len(String/StringSlice)` is not supported
# `len` over a `ref String` field converts the field to a `StringSlice`
# borrowing the field's referent and reaches upstream's `@unavailable`
# overload, as `len` over a local `String` does.
struct H[o: MutOrigin]:
    var r: ref[Self.o] String

    def __init__(out self, ref[Self.o] r: String):
        self.r = r


def main():
    var s = String("abc")
    var h = H(s)
    print(len(h.r))
