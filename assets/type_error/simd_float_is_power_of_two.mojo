# `SIMD.is_power_of_two` is its declared member, whose `comptime assert`
# fails a floating-point instance, as at the pin.
# expect: constraint failed: must be integral
def main():
    var f = SIMD[DType.float32, 2](1.0, 2.0)
    print(f.is_power_of_two())
