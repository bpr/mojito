# A generic body over a hand-written `EnumLike` conformer and a callable
# parameter reads the case list (`ParameterList[T._enum_case_names]()`) and
# calls the callable per case in a `comptime for`, with a plain and a
# generic callable. `print` of a conformer without `write_to` goes through
# `Writable`'s default, and a taken `comptime if` arm that returns skips a
# reflection loop over the conformer's fields.
from std.builtin.variadics import TypeList, _MLIR
from std.utils import IndexList


struct Shape(EnumLike, Writable):
    var tag: Int

    comptime _enum_case_names = ParameterList.of[
        "Empty".value, "Circle".value, "Square".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Int, Int, Int
    ].values

    def __init__(out self, tag: Int):
        self.tag = tag

    def _get_enum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.tag).unsafe_origin_cast[origin_of(self)]()[]
        )


def inc(x: Int) -> Int:
    return x + 10


def h[T: EnumLike, //, f: def(Int) thin -> Int](this: T) -> Int:
    comptime names = ParameterList[T._enum_case_names]()
    var total = 0
    comptime for i in range(names.length):
        total += f(i)
    return total


def show[F: Writable](field: F) -> String:
    return String(field)


def g[
    T: EnumLike, //, f: def[F: Writable](field: F) thin -> String
](this: T) -> String:
    comptime names = ParameterList[T._enum_case_names]()
    var out = String()
    var d = this._get_enum_discriminant()
    comptime for i in range(names.length):
        if d == i:
            out += StaticString(names[i])
            out += f(i)
    return out


@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Float64


def fields[T: AnyType](this: T):
    comptime if conforms_to(T, EnumLike):
        print("enum")
        return
    comptime r = reflect[T]
    comptime for i in range(r.field_count()):
        print(i, end=" ")
    print()


def main():
    print(h[inc](Shape(1)))
    print(g[show](Shape(1)))
    print(Shape(2))
    fields(Shape(0))
    fields(P(1, 2.0))
    var a = IndexList[3](1, 2, 3)
    var b = IndexList[3](4, 5, 6)
    var c = a + b
    print(c[0], c[1], c[2])
    print(a < b, b < a, a <= b, a == b)
