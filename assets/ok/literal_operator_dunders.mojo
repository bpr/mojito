# `IntLiteral` and `FloatLiteral` declare their operator and comparison
# dunders, so a literal receiver's explicit dunder call resolves, and an
# operator over two literals is the declared dunder, whose type carries the
# value its `#pop<int_literal_bin>` attribute folds.
def main():
    comptime N = 4
    print((2).__add__(3), (2).__floordiv__(3), (2).__lt__(3), N.__add__(3))
    print((5).__sub__(7), (6).__mul__(7), (7).__mod__(3), (1).__lshift__(3))
    print((12).__and__(10), (12).__xor__(10), (12).__or__(10), (16).__rshift__(2))
    print((2).__eq__(2), (2).__ne__(2), (2).__gt__(3), (3).__ge__(3), (3).__le__(2))
    print((2.5).__add__(1.0), (2.5).__lt__(3.0), (7.0).__truediv__(2.0))
    print((7.0).__floordiv__(2.0), (7.5).__mod__(-2.0), (2.5).__add__(2))
    print((0).__bool__(), (3).__bool__(), (0.0).__bool__(), (2.5).__bool__())
    print(2 + 3, 7 // 2, 7 % 3, 1 << 3, 2 < 3, 2 != 3, 5 - 9, 12 & 10)
    print(2.5 + 1.0, 7.0 / 2.0, 2.5 + 2, 7.5 % 2.0, 2.5 >= 2, N + 1, N * N)
    var x: type_of(2 + 3) = {}
    var y: type_of(2.5 % 2) = {}
    var z: type_of(N * N) = {}
    print(x, y, z, +N)
    if 3:
        print("nonzero")
