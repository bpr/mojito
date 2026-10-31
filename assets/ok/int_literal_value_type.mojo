# A numeric literal's type is the application `IntLiteral[value]` or
# `FloatLiteral[value]` it names, and a literal-only expression's type
# carries the value it folds to: `type_of` of either is that type, and a
# brace `{}` at it constructs the one value it names. Two literals of
# different values, and an annotated `comptime M: Int`, materialize.
comptime N = 4
comptime M: Int = 4
comptime F = 2.5
comptime S = N + 2.5


struct Holder:
    var x: type_of(F)

    def __init__(out self):
        self.x = {}


def main():
    var y: type_of(N) = 4
    var b: type_of(N) = {}
    var z: type_of(M) = 9
    var three: type_of(3) = 3
    print(y, b, z, three, Holder().x)
    var s: type_of(S) = {}
    var f: type_of(4.0) = {}
    print(s, f, y + 1)
    var a: type_of(N + 3) = {}
    comptime K = N * 2 - 1
    var k: type_of(K) = 7
    var c: type_of(-3) = -3
    var d: type_of(7 // 2) = 3
    var e: type_of(1 / 4) = 0.25
    var g: type_of(2**10) = 1024
    var h: type_of(1 << 3) = 8
    print(a, k, c, d, e, g, h)
    var cond = True
    comptime A = 1
    comptime B = 2
    var l = [A, B, 3]
    comptime C = 1 if True else 2
    var j: type_of(C) = 1
    print(A if cond else B, l[2], j, 1 if cond else 2)
    var i: type_of(4) = IntLiteral[(4).value]()
    print(i, IntLiteral[(4).value]())
