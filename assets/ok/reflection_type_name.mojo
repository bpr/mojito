# `reflect[T].name()` and `reflect[T].base_name()` are upstream's
# `#kgen.get_type_name` and `#kgen.get_base_type_name` wrapped in a
# `StaticString`: a struct qualified by its declaring module (this file's
# stem), the library's `SIMD`, `Bool`, `String`, and `List` bare unless
# `qualified_builtins`, each argument spelled inside the brackets. A
# subject that is still a parameter is answered per instance.
from std.collections import Set


struct Point:
    var x: Int
    var y: Float64


struct Box[T: AnyType, n: Int]:
    var a: Int


struct Keyed[d: DType, flag: Bool, b: UInt8]:
    pass


struct Holder[T: AnyType]:
    def __init__(out self):
        pass

    def show(self):
        print(reflect[Self].name(), reflect[Self.T].name())


def show[T: AnyType]():
    print(reflect[T].name(), reflect[T].base_name())


def main():
    print(reflect[Point].name())
    print(reflect[Box[Point, 3]].name(), reflect[Box[Point, 3]].base_name())
    print(reflect[Keyed[DType.int8, False, 200]].name())
    print(reflect[Int].name(), reflect[Int].base_name())
    print(reflect[Int].name[qualified_builtins=True]())
    print(reflect[List[Optional[Int]]].name())
    print(reflect[List[Optional[Int]]].name[qualified_builtins=True]())
    print(reflect[Dict[Int, Int]].name())
    print(reflect[Set[String]].base_name())
    print(reflect[SIMD[DType.float32, 4]].name(), reflect[Bool].name())
    print(reflect[DType].name(), reflect[NoneType].name(), reflect[Error].name())
    show[Point]()
    show[Box[Int, 2]]()
    Holder[Holder[Bool]]().show()
    comptime name = reflect[Point].name()
    var held = reflect[Float64].name()
    print(name, held, held.byte_length())
