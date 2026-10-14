# A copy of a register-passable trivial value is the loaded value itself: a
# vector SIMD field, a `TrivialRegisterPassable` struct field, or a
# `StringLiteral` local, copied out and then stored over, keeps the value it
# was read with. An owned callee that mutates its argument does not reach the
# caller's copy, and a by-value argument is read at call setup, after a later
# argument has written its place.
@fieldwise_init
struct P(TrivialRegisterPassable, Writable):
    var x: Int
    var y: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P(", self.x, ", ", self.y, ")")


@fieldwise_init
struct Box(Movable):
    var v: SIMD[DType.int32, 4]
    var p: P


def take_v(var v: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    v[0] = 99
    return v


def take_p(var p: P) -> Int:
    p.x = 99
    return p.x


def get_v(b: Box) -> SIMD[DType.int32, 4]:
    return b.v


def get_p(b: Box) -> P:
    return b.p


def bump(mut b: Box) -> Int:
    b.p = P(9, 9)
    return 0


def main():
    var b = Box(SIMD[DType.int32, 4](1, 2, 3, 4), P(1, 2))
    var w = get_v(b)
    var v = b.v
    b.v = SIMD[DType.int32, 4](5, 6, 7, 8)
    print(take_v(v), v, w, b.v)
    var p = b.p
    var q = get_p(b)
    b.p = P(7, 8)
    print(take_p(p), p, q, b.p)
    var r = get_p(b)
    _ = bump(b)
    print(r, b.p)
    b.p = P(1, 2)
    print(b.p, bump(b))
    var l = "abc"
    var m = l
    l = "xyz"
    print(m, l)
