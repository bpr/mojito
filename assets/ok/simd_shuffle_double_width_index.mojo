# A one-operand `shuffle[*mask]()` shuffles the receiver with itself, so a
# mask index may reach up to twice the width: `_shuffle_list` bounds each
# index by `2 * Self.length`, as at the pin.


def main():
    print(SIMD[DType.int32, 2](5, 6).shuffle[3, 0]())
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v.shuffle[4, 5, 6, 7]())
    print(v.shuffle[7, 0, 5, 2]())
