# A parameterized `comptime` body is evaluated under its binders, and a type
# result is a type, whatever the body's spelling: a struct member whose
# conditional body evaluates to a type witnesses a trait's type requirement,
# and a generic alias's conditional body is a type at each application.


trait HasU:
    comptime U[n: Int]: AnyType


struct S(HasU):
    comptime U[n: Int] = Int if n > 0 else Float64


comptime Pick[T: AnyType, U2: AnyType, b: Bool] = T if b else U2


def main():
    var a: S.U[1] = 3
    var b: S.U[0] = 2.5
    var c: Pick[Int, Float64, True] = 4
    var d: Pick[Int, Float64, False] = 1.5
    print(a, b, c, d)
