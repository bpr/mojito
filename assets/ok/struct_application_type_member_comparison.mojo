# A struct application's type member is a type operand of a compile-time
# comparison: `S[Int].A == Int` reads `A` through the application, in a
# local and a module `comptime`, a bracket argument, and a generic body,
# over a single and a variadic parameter list.


struct S[T: Movable]:
    comptime A = Int
    comptime B = Self.T


struct V[*Ts: Movable]:
    comptime A = Int


comptime m = S[Int].A == Int
comptime n = S[Bool].B == Bool


def materialize[b: Bool]() -> Bool:
    return b


def same[T: Movable]() -> Bool:
    return materialize[S[T].B == T]()


def main():
    comptime b = S[Int].A == Int
    comptime c = S[Int].A == Float64
    print(b, c, m, n)
    print(materialize[S[Int].A == Int](), materialize[S[Int].B != Int]())
    print(materialize[V[Int, Bool].A == Int](), same[Bool]())
