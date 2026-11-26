# A three-word struct whose pointer field's bytes are overwritten with text
# through a byte view of the struct, as an inline `String` holds its bytes:
# the pointer word is copied, moved, and held in a `List` as plain bits.


struct Packed(Copyable, Movable):
    var a: Pointer[UInt8, MutUntrackedOrigin]
    var b: Int
    var c: Int

    def __init__(out self):
        self.a = {unsafe_from_address = Int(0)}
        self.b = 0
        self.c = 1 << 63

    def __init__(out self, *, copy: Self):
        self.a = copy.a
        self.b = copy.b
        self.c = copy.c

    def length(self) -> Int:
        return Int(UInt64(self.c & (0x1F << 56)) >> UInt64(56))

    def push(mut self, b: UInt8):
        var n = self.length()
        var p = Pointer(to=self).unsafe_bitcast[UInt8]()
        p[unsafe_offset=n] = b
        self.c = (self.c & ~(0x1F << 56)) | ((n + 1) << 56)

    def get(self, i: Int) -> UInt8:
        return Pointer(to=self).unsafe_bitcast[UInt8]()[unsafe_offset=i]

    def text(self) -> String:
        var out = String()
        for i in range(self.length()):
            out.write(Codepoint(self.get(i)))
        return out^


def main():
    var s = Packed()
    for i in range(20):
        s.push(UInt8(97 + i))
    print(s.length(), s.text())
    var t = s.copy()
    t.push(UInt8(66))
    print(t.text(), s.text())
    var xs = List[Packed]()
    xs.append(Packed())
    xs[0].push(UInt8(88))
    xs[0].push(UInt8(89))
    xs.append(t.copy())
    print(xs[0].text(), xs[1].length())
    var m = s^
    m.push(UInt8(67))
    print(m.text(), m.length())
