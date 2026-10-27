# A compile-time member read through a construction reads the member on
# the type the construction builds.

struct Types[*Ts: AnyType]:
    comptime _types = Self.Ts.values
    comptime n = TypeList[Trait=AnyType, Self._types]().length


struct Direct[*Ts: AnyType]:
    comptime n = TypeList[Trait=AnyType, Self.Ts.values]().length


comptime Count[*Ts: AnyType] = TypeList[Trait=AnyType, Ts.values]().length


def local_count[*Ts: AnyType]() -> Int:
    comptime n = TypeList[Ts.values]().length
    return n


def returned_count[*Ts: AnyType]() -> Int:
    return TypeList[Ts.values]().length


def main():
    print(Types[Int, Bool].n)
    print(Direct[Int, Bool].n)
    print(Count[Int, Bool, Float64])
    print(local_count[Int, Bool, Int]())
    print(returned_count[Int, Bool, Int]())
    print(TypeList.of[Trait=AnyType, Int, Bool]().length)
