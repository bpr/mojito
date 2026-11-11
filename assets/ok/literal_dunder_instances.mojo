# Each literal is its own type, so `(-0.5).__neg__` and `(-709.4).__neg__`,
# or `2.5.__mul__(3.0)` and `1.5.__mul__(3.0)`, are distinct instances
# with distinct results, at compile time as at run time.


def g() -> Float64:
    var a: Float64 = 2.5 * 3.0
    var b: Float64 = 1.5 * 3.0
    var c: Float64 = -0.5
    var d: Float64 = -709.4
    return a + b + c + d


def main():
    comptime c = g()
    print(c, g())
    var m = SIMD[DType.bool, 4](True, False, True, False)
    print(~m, -3, ~3)
