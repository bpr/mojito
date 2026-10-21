# A trait's signature reads a `comptime` member of a struct declared in the
# same module, before or after the trait, as std's `EnumLike` reads
# `_MLIR.KGENParamListType`.


struct M:
    comptime X = Int
    comptime KGENTypeType = __mlir_type.`!kgen.type`
    comptime KGENParamListType[elt_type: Self.KGENTypeType] = __mlir_type[
        `!kgen.param_list<`, elt_type, `>`
    ]


trait Before:
    def f(self, x: M.X) -> Int:
        ...


trait After:
    def g(self, x: N.Y) -> Int:
        ...


trait Listed:
    comptime types: M.KGENParamListType[AnyType]
    comptime count: Int = 7

    def h(self) -> Int:
        ...


struct N:
    comptime Y = Int


@fieldwise_init
struct Both(After, Before, Listed):
    var base: Int
    comptime types = TypeList.of[Trait=AnyType, Int, Bool].values

    def f(self, x: Int) -> Int:
        return self.base + x

    def g(self, x: Int) -> Int:
        return self.base * x

    def h(self) -> Int:
        return Self.count


def length[T: Listed]() -> Int:
    return TypeList[Trait=AnyType, T.types]().length


def main():
    var b = Both(3)
    print(b.f(4), b.g(5), b.h(), Both.count, length[Both]())
