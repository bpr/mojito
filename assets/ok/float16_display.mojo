# A `Float16` prints its value. Upstream renders it at single precision
# (`0.7998047`); Mojito prints the exact double view, as it does for `Float32`.
def main():
    var a = Float16(0.8)
    print(a, String(a))
    print(Float16(0.1), Float16(1.0 / 3.0), Float16(65504.0))
    for x in range(Float16(0.5), Float16(2.0), Float16(0.3)):
        print(x)
    print(SIMD[DType.float16, 2](1.0, 0.1))
