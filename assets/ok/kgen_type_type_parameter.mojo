# A parameter typed by upstream's KGEN metatype `!kgen.type`
# (`__TypeOfAllTypes`), spelled directly, through a module alias, or through
# a struct's member alias, binds any type, forwarded `AnyType` binders and
# the metatype itself included.
comptime KT = __mlir_type.`!kgen.type`


struct Meta:
    comptime KGENTypeType = __mlir_type.`!kgen.type`
    comptime ListOf[elt_type: Self.KGENTypeType] = __mlir_type[
        `!kgen.param_list<`, elt_type, `>`
    ]


def direct[T: __mlir_type.`!kgen.type`]() -> Int:
    return 1


def aliased[T: KT]() -> Int:
    return 2


def member[T: Meta.KGENTypeType]() -> Int:
    return 3


def forward[U: AnyType]() -> Int:
    return direct[U]() + aliased[U]() + member[U]()


def take[T: __mlir_type.`!kgen.type`](x: T) -> Int:
    return 4


def count[t: AnyType, //, raw: Meta.ListOf[t]]() -> Int:
    return ParameterList[raw].size


def pack[*vs: Int]() -> Int:
    return count[vs.values]()


def main():
    print(direct[Int](), aliased[String](), member[Bool]())
    print(forward[Float64]())
    print(member[Meta.KGENTypeType]())
    print(take[Int](7))
    print(pack[5, 6, 7]())
