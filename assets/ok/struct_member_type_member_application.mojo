# A struct's type-valued parameterized `comptime` member applies through
# `Self` in a compile-time expression: in a later member (`comptime First =
# Self.Get[0]`, `Self.E[Self.n]`) and in a method's local `comptime` and
# `comptime if`. A type pack's element spelled as its `TypeList` member,
# `Self.Ts.__getitem_param__[0]`, is the same read as `Self.Ts[0]`.


struct W[T: AnyType]:
    comptime Get[i: Int] = Self.T
    comptime First = Self.Get[0]

    def __init__(out self):
        pass

    def m(self):
        comptime X = Self.Get[0]
        comptime if Self.Get[0] == Int:
            print("int")
        comptime if X == Int:
            print("x int")


struct Box[k: Int]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.k)


struct S:
    comptime n = 2
    comptime E[k: Int] = Box[k]
    comptime F = Self.E[Self.n]


struct V[T: AnyType, n: Int]:
    comptime Get[i: Int] = SIMD[DType.int32, i]
    comptime First = Self.Get[Self.n]


struct P[*Ts: AnyType]:
    comptime First = Self.Ts.__getitem_param__[0]
    comptime Second = Self.Ts[1]


def main():
    comptime if W[Int].First == Int:
        print("first int")
    comptime E = W[Bool].First
    var y: E = True
    print(y)
    W[Int]().m()
    var b: S.F = S.F()
    b.show()
    var v: V[Int, 4].First = V[Int, 4].First(1)
    print(v)
    comptime if V[Bool, 2].First == SIMD[DType.int32, 2]:
        print("same")
    var x: P[Int, Bool].First = 3
    print(x)
    comptime if P[Int, Bool].First == Int and P[Int, Bool].Second == Bool:
        print("yes")
