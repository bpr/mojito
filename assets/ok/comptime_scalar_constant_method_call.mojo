# A method called on a module `comptime` constant of a builtin scalar type
# dispatches to the annotated type's member: `X` is the `Int`, never the
# literal it was written as, at module scope and in a body.
comptime X: Int = 12
comptime W = X.__neg__()
comptime Y = X.__and__(5)
comptime SMALL: UInt8 = 5
comptime HALF: Float64 = 1.5
comptime FLAG: Bool = True
comptime COUNT: UInt = 7


def main():
    print(X.__neg__())
    print(W, Y)
    comptime W2 = X.__neg__()
    comptime Y2 = X.__and__(5)
    print(W2, Y2)
    var negated = X.__neg__()
    print(negated)
    print(SMALL.__neg__())
    print(HALF.__neg__())
    print(FLAG.__invert__())
    print(COUNT.__and__(3))
