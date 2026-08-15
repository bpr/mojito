# A struct DType value parameter may declare upstream's `= DType.int`
# default (the declaration compiles the default's `DType.<dt>` member
# expression). Applications still bind the parameter explicitly — bare
# default application on a comptime-class struct stays a recorded residue.
struct Box[dtype: DType = DType.int]:
    var x: Scalar[dtype]

    def __init__(out self, x: Scalar[dtype]):
        self.x = x

def main():
    var a = Box[DType.int32](Int32(7))
    print(a.x)
    var b = Box[DType.int](9)
    print(b.x)
