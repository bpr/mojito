# Type applications as type-pack elements: `TypeNames` accepts a nested
# `Tuple[...]`, a `SIMD[...]` vector, and an `Optional` over a Tuple, and a
# Tuple with a SIMD element default-constructs. (Upstream prints
# `Tuple[<unprintable>, {}]` for the Tuple element, so this fixture has no
# differential row.)
from std.format._utils import TypeNames

def main():
    print(TypeNames[Tuple[Int, Bool]]())
    print(TypeNames[SIMD[DType.int, 4]]())
    print(TypeNames[Optional[Tuple[Int, Bool]]]())
    print(TypeNames[Int, Tuple[Int, Tuple[Int, Bool]]]())
    var s = Tuple[SIMD[DType.int, 4], Int]()
    print(s[0], s[1])
