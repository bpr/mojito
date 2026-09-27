# A struct keyed on a closed vector value (`key: Pair`, a module alias of
# `SIMD[DType.uint64, 2]`), specialized whole, as the bundled `AHasher` is.
# Its members read the key (`Self.key`), construct through the alias
# (`Pair(1, 2)`), copy a lane into a field, read the module's integer
# constants (`UInt64(MULTIPLE)`, `64 - ROT`), and call a module function
# over closed values, and derive from their checked templates at every key.
comptime Pair = SIMD[DType.uint64, 2]
comptime MULTIPLE = 6364136223846793005
comptime ROT = 23


def _mix(lhs: UInt64, rhs: UInt64) -> UInt64:
    return (lhs * rhs) ^ (lhs >> UInt64(32))


struct Mixer[key: Pair](Movable):
    var state: UInt64
    var extra: UInt64

    def __init__(out self):
        var keyed = Self.key ^ Pair(0x243F6A8885A308D3, 0x13198A2E03707344)
        self.state = keyed[0]
        self.extra = keyed[1]

    def feed(mut self, value: UInt64):
        self.state = _mix(value ^ self.state, UInt64(MULTIPLE))

    def rotate(mut self):
        var mixed = self.state + self.extra
        self.state = (mixed << UInt64(ROT)) | (mixed >> UInt64(64 - ROT))

    def finish(var self) -> UInt64:
        return self.state ^ self.extra


def main():
    var zero = Mixer[Pair(0)]()
    zero.feed(UInt64(7))
    zero.rotate()
    print(zero^.finish())
    var keyed = Mixer[Pair(3, 4)]()
    keyed.feed(UInt64(7))
    keyed.rotate()
    print(keyed^.finish())
