# `EnumLike` is the prelude's upstream trait. A hand-written conformer,
# `Optional`, and `OptionalReg` run one generic `T: EnumLike` body that reads
# the discriminant, both case lists' lengths, `_enum_is_exhaustive`, and a
# payload. The pin validates no case metadata at the declaration, so an open
# enum with three names over two types runs, as does a one-case enum.
from std.builtin.variadics import TypeList, _MLIR
from std.collections import OptionalReg


struct Shape(EnumLike):
    var tag: Int
    var r: Float64

    comptime _enum_case_names = ParameterList.of[
        "Empty".value, "Circle".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Float64
    ].values

    def __init__(out self, tag: Int, r: Float64):
        self.tag = tag
        self.r = r

    def _get_enum_discriminant(self) -> Int:
        return self.tag

    def get(ref self) -> ref[self.r] Float64:
        return self.r

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        comptime assert id != 0, "cannot get payload for Empty case"
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.get()).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Open(EnumLike):
    var n: Int

    comptime _enum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, n: Int):
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        return 1

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Unit(EnumLike):
    comptime _enum_case_names = ParameterList.of["Only".value].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType
    ].values

    def __init__(out self):
        pass

    def _get_enum_discriminant(self) -> Int:
        return 0

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        comptime assert False, "a unit enum has no payload"
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](self)


def show[T: EnumLike](ref x: T) -> Int:
    print(
        x._get_enum_discriminant(),
        ParameterList[T._enum_case_names]().size,
        TypeList[Trait=AnyType, T._enum_case_types]().length,
        T._enum_is_exhaustive,
    )
    return x._get_enum_discriminant()


def payload1[
    T: EnumLike
](ref x: T) -> ref[x] TypeList[Trait=AnyType, T._enum_case_types]()[1]:
    return x._unsafe_get_enum_payload[1]()


def main():
    var s = Shape(1, 2.5)
    var o = Optional(3)
    var e = Optional[Int]()
    _ = show(s)
    _ = show(o)
    _ = show(e)
    print(payload1(s), payload1(o))
    payload1(o) = 8
    print(o.value())
    ref q = o._unsafe_get_enum_payload[1]()
    q += 1
    print(o.value())
    var z: Optional[String]._enum_elt_type_for_case[1] = "hi"
    print(z)
    var r = OptionalReg[Int](4)
    _ = show(r)
    print(payload1(r))
    var w = Open(6)
    _ = show(w)
    print(payload1(w))
    _ = show(Unit())
    comptime if conforms_to(Optional[String], EnumLike):
        print("Optional[String] is EnumLike")
