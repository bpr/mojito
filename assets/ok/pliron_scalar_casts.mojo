# Width-1 dtype casts: integer targets rewrap at the new width (truncate, or
# sign/zero-extend by the source's signedness), float targets convert through
# f64 with Float32 rounding, and a float source truncates toward zero
# saturating at the 128-bit intermediate before wrapping — so a huge
# magnitude wraps to its low bits instead of clamping at the target width.
def main():
    var a = Int32(-2)
    print(a.cast[DType.uint8]()) # 254
    var d = UInt16(65535)
    print(d.cast[DType.int8]()) # -1
    print(Int8(-5).cast[DType.int64]()) # sign-extends
    print(UInt8(200).cast[DType.int64]()) # zero-extends
    print(Int8(-5).cast[DType.float32]())
    print(Int8(-5).cast[DType.float64]())
    var big = Float32(1e30)
    print(big.cast[DType.uint8]()) # low bits: 0
    print(big.cast[DType.int64]()) # low bits: 0
    print(Float32(-1.9).cast[DType.int32]()) # truncates toward zero: -1
    print(Float32(300.5).cast[DType.uint8]()) # wraps: 44
    var w = Float32(300.7)
    print(w.cast[DType.float64]())
    var back = Float64(300.7)
    print(Float32(back)) # rounds to binary32
    var b: Byte = Byte(200)
    print(b.cast[DType.int8]()) # -56
    print(Byte(300)) # construction wraps: 44
    print(Int32(Byte(200))) # 200
