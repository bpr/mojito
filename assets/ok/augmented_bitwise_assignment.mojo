# Bitwise augmented assignment works on builtin integers and dispatches to the
# dedicated in-place dunder on nominal values.
@fieldwise_init
struct Bits(Writable):
    var value: Int

    def __iand__(mut self, other: Self):
        self.value &= other.value

    def __ior__(mut self, other: Self):
        self.value |= other.value

    def __ixor__(mut self, other: Self):
        self.value ^= other.value

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.value)

def main():
    var signed: Int = 12
    signed &= 10
    signed |= 3
    signed ^= 6
    print(signed)

    var unsigned: UInt = UInt(12)
    unsigned &= UInt(10)
    unsigned |= UInt(3)
    unsigned ^= UInt(6)
    print(unsigned)

    var bits = Bits(12)
    bits &= Bits(10)
    bits |= Bits(3)
    bits ^= Bits(6)
    print(String(bits))
# stdout: 13
# stdout: 13
# stdout: 13
