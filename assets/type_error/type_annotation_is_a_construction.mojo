# A `TypeList` construction is a compile-time value, not a type: only its
# subscript is a type, so annotating a parameter with it is rejected.
# expect: type mismatch for type annotation: expected a type, found a value
def second[*Ts: AnyType](ref x: TypeList[Ts.values]()) -> Int:
    return 7


def main():
    var n = 3
    print(second[Int](n))
