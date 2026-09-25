# A per-instantiation method clone inherits its checked template's facts when
# the body computes over closed `SIMD` values (`docs/notes/instantiation-from-
# template.md`, class MethodBody): a `UInt64` local mixed through operators
# and augmented assignments, a scalar field stored back, a `UInt32` counter
# tested by a `while` and an `Int32` by an `if`, whose bool-lane conditions
# convert through `Bool(x)` alike in every instance, a closed vector field
# doubled in place, and a `UInt64` passed by value to a sibling method. A
# closed type is the same under every instance, so each derives for an
# `Int` and a `String` instance.
from std.hashlib import Hasher


struct Mixer[T: Copyable & Deinitable & Hashable & Writable](Hashable, Movable):
    var value: Self.T
    var salt: UInt64
    var lanes: SIMD[DType.uint8, 4]

    def __init__(out self, var value: Self.T, salt: UInt64):
        self.value = value^
        self.salt = salt
        self.lanes = SIMD[DType.uint8, 4](1, 2, 3, 4)

    def __hash__[H: Hasher](self, mut hasher: H):
        var c = UInt64(0)
        c ^= UInt64(5)
        c = ((c ^ 89869747) ^ (c << 16)) * 3644798167
        c += self.salt
        hasher._update_with_simd(c)
        self.value.__hash__(hasher)

    def resalt(mut self, step: UInt64):
        var s = self.salt
        s = (s << 3) | (s >> 61)
        s ^= step
        self.salt = s

    def countdown(self) -> Int:
        var n = UInt32(3)
        var steps = 0
        while n != 0:
            n -= 1
            steps += 1
        var error = Int32(0)
        if error != 0:
            steps += 100
        return steps

    def double(mut self):
        var v = self.lanes
        v += v
        self.lanes = v

    def fold(self, x: UInt64) -> Int:
        return Int(x ^ self.salt)

    def folded(self) -> Int:
        var x = UInt64(9)
        return self.fold(x)


def main():
    var a = Mixer[Int](3, UInt64(1))
    var b = Mixer[String](String("x"), UInt64(2))
    print(hash(a) == hash(a), hash(a) == hash(b))
    a.resalt(UInt64(7))
    b.resalt(UInt64(7))
    print(a.salt, b.salt)
    print(a.countdown(), b.countdown())
    a.double()
    b.double()
    b.double()
    print(a.lanes, b.lanes)
    print(a.folded(), b.folded())
