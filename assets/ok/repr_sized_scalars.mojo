# `repr` of a scalar names its type as upstream's `SIMD.write_repr_to` does:
# the scalar alias of a width-1 `SIMD` (`Int8(3)`, `Float32(0.5)`), `Int`
# and `Float64` included, around the lane's text.
def main():
    print(repr(Int8(3)), repr(Int16(-4)), repr(Int32(-5)), repr(Int64(6)))
    print(repr(UInt8(7)), repr(UInt16(8)), repr(UInt32(9)), repr(UInt64(10)))
    print(repr(Float16(1.5)), repr(Float32(0.5)), repr(Float64(2.5)))
    var x: Int32 = -12
    var f: Float32 = 0.25
    print(repr(x), repr(f), repr(Int(4)), repr(UInt(11)))
