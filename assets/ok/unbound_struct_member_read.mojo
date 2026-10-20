# A `comptime` member that does not depend on a struct's parameters reads
# through the struct left unbound, as a value, a type, and an application;
# a default does not bind a parameter there.
struct S[T: AnyType, k: Int]:
    comptime n = 3
    comptime plus[j: Int] = j + 1
    comptime Of[U: AnyType] = Tuple[U, Int]


struct D[T: AnyType = Int]:
    comptime width = 8


def main():
    comptime m = S.n
    print(m)
    print(S.n * 2)
    print(S.plus[4], S.plus[j=5])
    var d: S.Of[Bool] = (True, 6)
    print(d[1])
    comptime B = S.Of[Int]
    var e: B = (7, 8)
    print(e[0])
    print(D.width)
