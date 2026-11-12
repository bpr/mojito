# A sized scalar name in a bracket is one expression, so a member read off
# it (`Int8.MAX`, `Int32.dtype`) is a compile-time value `materialize` takes,
# and `Int`, `UInt8`, and `Bool` operands still name their types.


def main():
    print(materialize[Int8.MAX]())
    print(materialize[Int32.dtype]())
    print(materialize[UInt8.MIN]())
    var xs = List[Int]()
    xs.append(3)
    print(xs[0])
    var bytes = List[UInt8]()
    bytes.append(7)
    print(bytes[0])
    var flags = List[Bool]()
    flags.append(True)
    print(flags[0])
