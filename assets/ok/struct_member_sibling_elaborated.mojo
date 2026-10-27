# A compile-time binding of a struct member whose body reads a sibling
# member through `Self` is elaborated, through an instance, the unbound
# struct, a bracket value, and a parameterized member, a trait's default
# among the siblings.
struct S[T: AnyType]:
    comptime a = 2
    comptime b = Self.a + 1


struct Box[n: Int]:
    comptime w = Self.n * 10


struct M:
    comptime n = 3
    comptime m = Box[Self.n].w


trait HasK:
    comptime K: Int = 100


struct A(HasK):
    comptime get[i: Int]: Int = i + Self.K


def main():
    comptime b = S[Int].b
    comptime unbound = S.b
    print(b, unbound)
    comptime x = M.m
    print(x, M.m)
    comptime k = A.get[2] + 1
    print(k, A.get[2])
