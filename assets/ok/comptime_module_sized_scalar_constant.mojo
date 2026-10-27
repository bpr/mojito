# A closed module constant is the parameter expression the check builds,
# whatever its initializer spells: a sized scalar, a conditional, a call of
# a library function, an alias of another constant, a string
# concatenation, and a generic alias's application each fold to the value
# a body, a bracket argument, a `comptime if` or `comptime for`,
# `materialize`, `comptime(e)`, a local `comptime`, and a default read.

comptime K = Int32(4)
comptime W = UInt8(250)
comptime X = Float32(1.5)
comptime C = 3 + 1 if 3 > 2 else 0
comptime M = max(3, 4)
comptime N = M * 2
comptime K2 = K + 1
comptime A = 3
comptime B = A
comptime S = "buf" + "fer"


def twice(n: Int) -> Int:
    return n * 2


comptime Twice[n: Int] = twice(n)
comptime Z = Twice[6]


def show[n: Int]():
    print(n)


def scaled(x: Int32 = K * 2) -> Int32:
    return x


def main():
    print(K, W, X, C, M, N, K2, B, S, Z)
    show[C]()
    show[M]()
    comptime if C > 3:
        print("big")
    comptime for i in range(C):
        print(i)
    print(materialize[C](), comptime(C + 1))
    comptime k = K + 2
    print(k)
    print(scaled())
