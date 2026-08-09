# Elementwise dtype casts: integer targets re-wrap at the new element width,
# float targets convert numerically, and a float source truncates toward
# zero into integer lanes; a width-1 scalar alias casts too.
def main():
    var v = SIMD[DType.int32, 4](1, -2, 3, -300)
    print(v.cast[DType.uint8]())
    print(v.cast[DType.float32]())
    var f = SIMD[DType.float32, 2](2.9, -3.9)
    print(f.cast[DType.int32]())
    var b: Byte = Byte(200)
    print(b.cast[DType.int8]())
