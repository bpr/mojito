# Strided List slicing keeps `StridedSlice.indices()` normalization and the
# copied-List result: negative and out-of-range bounds wrap and clamp, and a
# negative step reverses. Only contiguous slices are strict.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[::-1])
    print(xs[-2::1])
    print(xs[-100:100:2])
    print(xs[3:0:-1])
