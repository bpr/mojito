# An index-keyed template's `pop.array.get` is checked against the array's
# size in each instance: `get[1]` is in bounds, and `get[4]` fails its
# instance.
# expect: function instantiation of `get` failed: 'pop.array.get' op array index out of bounds: 4
def get[i: Int](a: __mlir_type[`!pop.array<3, `, Int, `>`]) -> Int:
    return __mlir_op.`pop.array.get`[_type=Int, index=i.__mlir_index__()](a)


def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    print(get[1](a))
    print(get[4](a))
