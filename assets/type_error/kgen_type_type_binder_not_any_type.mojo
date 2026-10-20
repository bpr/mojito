# expect: type 'U' for parameter 'T' does not conform to trait 'AnyType'
# A `!kgen.type` binder implies no trait, not even `AnyType`, so it cannot
# instantiate `List[T: AnyType]`.
def h[U: __mlir_type.`!kgen.type`]() -> Int:
    var x = List[U]()
    return len(x)


def main():
    print(h[Int]())
