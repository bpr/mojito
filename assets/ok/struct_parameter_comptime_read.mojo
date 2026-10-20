# A struct's parameter read through a type (`S[7].n`) is a compile-time
# value in a module or local `comptime`, as at the pin, and so is a closed
# list's raw `values` (`TypeList.of[Trait=AnyType, NoneType, Int].values`).
struct S[n: Int]:
    pass


comptime c = S[7].n
comptime types = TypeList.of[Trait=AnyType, NoneType, Int].values


def main():
    comptime d = S[7].n + 1
    comptime local_types = TypeList.of[Trait=AnyType, Int, Bool, Int].values
    print(c, d)
    print(TypeList[types].length, TypeList[local_types].length)
