# A generic alias's variadic parameter binds as a pack: a spread of it in a
# type body, its `values` list, and a value body's reads, closed and
# forwarded from a caller's own pack, on a module alias and a struct member.

comptime T2[*Ts: Copyable] = Tuple[*Ts]
comptime L[*Ts: ImplicitlyCopyable & Writable & Deinitable] = TypeList[
    Trait=ImplicitlyCopyable & Writable & Deinitable, Ts.values
]
comptime First[*vs: Int] = vs[0]
comptime Len[*Ts: AnyType] = Ts.length
comptime two = Len[Int, Bool]


struct M:
    comptime T3[*Ts: Copyable] = Tuple[*Ts]
    comptime L[*Ts: AnyType] = TypeList[Trait=AnyType, Ts.values]


def second[*Ts: Copyable & Writable](t: T2[*Ts]) -> String:
    return String(t[1])


def first_of[*Ts: ImplicitlyCopyable & Writable & Deinitable](x: Int):
    var y: L[*Ts]()[0] = rebind[L[*Ts]()[0]](x)
    print(y)


def count[*Ts: AnyType]() -> Int:
    return Len[*Ts]


def main():
    var pair: T2[Int, String] = (1, "s")
    print(pair[0], second(pair))
    var trio: M.T3[Int, Int, Bool] = (5, 6, True)
    print(trio[2])
    var four: L[Bool, Int]()[1] = 4
    print(four)
    var five: M.L[Bool, Int]()[1] = 5
    print(five)
    first_of[Int, Bool](3)
    comptime head = First[7, 8, 9]
    print(head, First[1, 2], two)
    comptime if Len[Int, Bool, Int] == 3:
        print("three")
    print(count[Int, Bool, Int, Bool]())
