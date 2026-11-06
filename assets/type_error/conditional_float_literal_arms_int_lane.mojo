# expect: the SIMD type must be floating point
# Under `Int32`, each arm of `A if c else 0.25` converts through upstream's
# `@implicit SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails.
comptime A = 0.5


def main():
    var c = True
    var x: Int32 = A if c else 0.25
    print(x)
