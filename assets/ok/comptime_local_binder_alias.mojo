# A local `comptime` binding whose value names a compile-time parameter
# (`comptime k = n`, `comptime lane = dt`) is an alias of that parameter
# expression, checked once in the generic body's template: a read at runtime
# materializes the parameter, a read in a type or a `comptime if` is the
# parameter itself, and an alias of an alias names the same binder.


def is_even(x: Int) -> Bool:
    return x % 2 == 0


def f[n: Int]() -> Int:
    comptime k = n
    return k + 1


def parity[n: Int]() -> String:
    comptime m = n
    comptime if is_even(m):
        return "even"
    else:
        return "odd"


def g[dt: DType, n: Int]() -> Int:
    comptime m = n
    comptime lane = dt
    return m + Int(Scalar[lane](1))


def scale[dt: DType](x: Scalar[dt]) -> Scalar[dt]:
    comptime lane = dt
    comptime again = lane
    comptime if again.is_floating_point():
        return x * Scalar[again](2.5)
    else:
        return x + Scalar[lane](4)


def main():
    print(f[3](), f[4]())
    print(parity[1](), parity[2]())
    print(g[DType.int8, 2](), g[DType.float32, 5]())
    print(scale[DType.int32](3), scale[DType.float64](1.0))
