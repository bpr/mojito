# A read or write through a `Pointer.unsafe_bitcast` retype to a scalar or
# vector of one bit width reinterprets the bits at the address, as upstream's
# `pop.pointer.bitcast` and a typed load or store do: `UInt8` 200 reads as
# `Int8` -56, a write of -1 through the `Int8` view leaves 255, and a
# `Float64` reads as its `UInt64` bits.
from std.memory.alloc import unsafe_alloc


def show(x: Int8):
    print(x)


def bump(mut x: Int8):
    x += 1


def main():
    var x: UInt8 = 200
    var p = Pointer(to=x).unsafe_bitcast[Int8]()
    print(p[])
    p[] = -1
    print(x)

    var f: Float64 = 1.0
    print(Pointer(to=f).unsafe_bitcast[UInt64]()[])
    var i: Int64 = 4607182418800017408
    print(Pointer(to=i).unsafe_bitcast[Float64]()[])

    var a: Int32 = -1
    print(Pointer(to=a).unsafe_bitcast[UInt32]()[])
    var v = SIMD[DType.uint8, 4](1, 2, 255, 4)
    print(Pointer(to=v).unsafe_bitcast[SIMD[DType.int8, 4]]()[])
    print(Pointer(to=v).unsafe_bitcast[UInt32]()[])

    var h: UInt16 = 65535
    var ph = Pointer(to=h).unsafe_bitcast[Int16]()
    print(ph[] + 1, ph[] == -1)
    var d: Int = -1
    print(Pointer(to=d).unsafe_bitcast[UInt]()[])
    print(Pointer(to=d).unsafe_bitcast[Int64]()[])

    var buf = unsafe_alloc[UInt8](4)
    buf[unsafe_offset=0] = 250
    buf[unsafe_offset=1] = 251
    buf[unsafe_offset=2] = 252
    buf[unsafe_offset=3] = 253
    var s = buf.unsafe_bitcast[Int8]()
    print(s[], s[unsafe_offset=1], s[unsafe_offset=2], s[unsafe_offset=3])
    show(s[unsafe_offset=1])
    bump(s[unsafe_offset=1])
    print(buf[unsafe_offset=1])
    print(s[unsafe_offset=2].__abs__())
    s[unsafe_offset=3] = -128
    print(buf[unsafe_offset=3])
    s.unsafe_offset(2).unsafe_write(Int8(-2))
    print(buf[unsafe_offset=2])
    print(s.unsafe_offset(2).unsafe_take_pointee())
    buf.unsafe_free()

    var g: Float32 = 1.5
    var pg = Pointer(to=g).unsafe_bitcast[UInt32]()
    print(pg[])
    pg[] = 1065353216
    print(g)

    var y: UInt8 = 200
    var py = Pointer(to=y)
    var raw = __mlir_op.`pop.pointer.bitcast`[
        _type=Pointer[Int8, origin_of(y)]._mlir_type
    ](py._mlir_value)
    print(Pointer[Int8, origin_of(y)](_mlir_value=raw)[])
