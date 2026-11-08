# `TypeList.__getitem_param__` takes a `SIMDLength` index, as upstream's
# `#kgen.param_list.get` body over `idx._mlir_value` does: an `Int` index
# converts through `SIMDLength.__init__(Int)`, a `SIMDLength` value indexes
# directly, and the member reads spelled out in a compile-time expression
# (`Ts.__getitem_param__[1]`, `comptime if Ts.__getitem_param__[0] == Int`)
# are its value. A parameterized member whose value is a type is read as a
# type in an annotation, whether a `SIMDLength` or a raw `index` takes it.
from std.builtin.variadics import TypeList, _MLIR


struct RawList[Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]]:
    comptime _mlir_type = _MLIR.KGENParamListType[Self.Trait]
    comptime get[idx: SIMDLength] = __mlir_attr[
        `#kgen.param_list.get<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `, `,
        idx._mlir_value,
        `> : `,
        +Self.Trait,
    ]


struct Holder[*Ts: Writable & Movable]:
    comptime Second = Self.Ts[1]


def count[*Ts: Movable]() -> Int:
    comptime First = Ts[0]
    comptime Next = Ts.__getitem_param__[1]
    comptime G = TypeList[Ts.values].__getitem_param__[1]
    comptime if Ts.__getitem_param__[0] == Int:
        print("Int first")
    return Ts.length


def third[*Ts: Writable & Movable](*args: *Ts):
    comptime i = 2
    comptime U = Ts[i]
    print(args[i])


def shown[*Ts: AnyType](x: RawList[Ts.values].get[1]):
    print("shown")


def take[
    *Ts: AnyType
](x: TypeList[Ts.values]._get_type_at_index[__mlir_attr.`0:index`]):
    print("took")


def main():
    shown[Int, Bool](True)
    take[Int](5)
    print(count[Int, Bool]())
    third(1, "two", 3.5)
    var h: Holder[Int, String].Second = "s"
    print(h)
    comptime L = TypeList.of[Trait=AnyType, Int, Bool]()
    var b: L[1] = True
    print(b)
    comptime k = SIMDLength(1)
    var c: L[k] = False
    print(c)
    var n: L[SIMDLength(0)] = 7
    print(n)
