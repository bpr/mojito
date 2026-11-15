# `SumType`, `EnumLike`, and `UnionLike` are the prelude's traits. A
# hand-written `UnionLike` conformer, `Optional`, `OptionalReg`, a
# hand-written `EnumLike` conformer, and `Bool` run one generic
# `T: SumType` body that reads the discriminant, the case names' length,
# and `_enum_is_exhaustive`; the `UnionLike` ones also run a body reading
# the case types' length and a payload. No case metadata is validated at
# the declaration, so an open union with three names over two types runs,
# as does a one-case enum.
# The three traits are upstream's enum proposal's, which the pin does not
# declare (`docs/non-goals.md`).
from std.builtin.variadics import TypeList, _MLIR
from std.collections import OptionalReg


struct Shape(UnionLike):
    var tag: Int
    var r: Float64

    comptime _sum_case_names = ParameterList.of[
        "Empty".value, "Circle".value
    ].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Float64
    ].values

    def __init__(out self, tag: Int, r: Float64):
        self.tag = tag
        self.r = r

    def _get_sum_discriminant(self) -> Int:
        return self.tag

    def get(ref self) -> ref[self.r] Float64:
        return self.r

    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        comptime assert id != 0, "cannot get payload for Empty case"
        return rebind[TypeList[Trait=AnyType, Self._union_case_types]()[id]](
            Pointer(to=self.get()).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Open(UnionLike):
    var n: Int

    comptime _sum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Int
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, n: Int):
        self.n = n

    def _get_sum_discriminant(self) -> Int:
        return 1

    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._union_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Unit(EnumLike):
    comptime _sum_case_names = ParameterList.of["Only".value].values

    def __init__(out self):
        pass

    def _get_sum_discriminant(self) -> Int:
        return 0


def show[T: SumType](ref x: T) -> Int:
    print(
        x._get_sum_discriminant(),
        ParameterList[T._sum_case_names]().length,
        T._enum_is_exhaustive,
    )
    return x._get_sum_discriminant()


def payloads[T: UnionLike](ref x: T) -> Int:
    return TypeList[Trait=AnyType, T._union_case_types]().length


def payload1[
    T: UnionLike
](ref x: T) -> ref[x] TypeList[Trait=AnyType, T._union_case_types]()[1]:
    return x._unsafe_get_union_payload[1]()


def main():
    var s = Shape(1, 2.5)
    var o = Optional(3)
    var e = Optional[Int]()
    _ = show(s)
    _ = show(o)
    _ = show(e)
    print(payloads(s), payloads(o))
    print(payload1(s), payload1(o))
    payload1(o) = 8
    print(o.value())
    ref q = o._unsafe_get_union_payload[1]()
    q += 1
    print(o.value())
    var z: Optional[String]._union_elt_type_for_case[1] = "hi"
    print(z)
    var r = OptionalReg[Int](4)
    _ = show(r)
    print(payload1(r))
    var w = Open(6)
    _ = show(w)
    print(payloads(w), payload1(w))
    _ = show(Unit())
    _ = show(True)
    _ = show(False)
    comptime if conforms_to(Optional[String], UnionLike):
        print("Optional[String] is UnionLike")
    comptime if conforms_to(Bool, EnumLike) and not conforms_to(Bool, UnionLike):
        print("Bool is EnumLike")
    comptime if conforms_to(Unit, SumType):
        print("Unit is a SumType")
