# `Tuple`'s default initializer is spelled once over the symbolic pack, each
# element built from its own default construction, so every Defaultable
# specialization derives it whatever its elements are: a scalar, a SIMD
# scalar, a `String`, a generic struct, a nested `Tuple`.
def main():
    var scalars = Tuple[Int, Bool, Float64]()
    print(scalars[0], scalars[1], scalars[2])
    var lanes = Tuple[UInt64, UInt8, Float32]()
    print(lanes[0], lanes[1], lanes[2])
    var text = Tuple[String, Int]()
    print(text[0].byte_length(), text[1])
    var maybe = Tuple[Optional[Int], Optional[String]]()
    print(maybe[0] is None, maybe[1] is None)
    var nested = Tuple[Tuple[Int, Bool], String]()
    print(nested[0][0], nested[0][1], nested[1].byte_length(), len(nested))
    var single = Tuple[Int]()
    print(single)
