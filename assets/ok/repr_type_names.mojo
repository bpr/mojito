# The repr vocabulary: `_unqualified_type_name[T]()` folds to current Mojo's
# unqualified type spelling at compile time, `TypeNames[...]()` writes a
# pack's names, and `repr` of a String is single-quoted.
from std.format._utils import TypeNames
from std.reflection.type_info import _unqualified_type_name

struct Point:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

def main():
    print(_unqualified_type_name[Int](), _unqualified_type_name[String](), _unqualified_type_name[Bool]())
    print(_unqualified_type_name[Float64](), _unqualified_type_name[Point](), _unqualified_type_name[List[Int]]())
    print(_unqualified_type_name[Optional[Int]](), _unqualified_type_name[Optional[String]]())
    print(TypeNames[Int, String]())
    print(TypeNames[Bool]())
    var word = String("hi")
    print(repr(word), repr(String("")))
