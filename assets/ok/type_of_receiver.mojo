# `type_of(value)` read as a receiver or a callee is the value's type: a
# static call and a member read through it are the type's, and a call of it
# constructs the type, here and through a `comptime` alias of it.
def f[T: Copyable & Defaultable](x: T) -> T:
    return type_of(x)()


def main():
    var t = (1, 2, 3)
    print(type_of(t).__len__())
    comptime T = type_of(t)
    print(T.__len__())
    var u = type_of(t)(4, 5, 6)
    print(u[2])
    var v = Float32(1)
    print(type_of(v)(Float32(2)))
    print(type_of(v).dtype)
    var s = SIMD[DType.int32, 4](1)
    print(type_of(s)(7))
    print(f(5))
