# A `var` in a template-served `comptime for` body whose type names the index
# (`SIMD[DType.int32, i]`, `Lanes[i]`) is one binding per iteration at that
# iteration's type, as each clone of upstream's unrolled region allocates its
# own: the elaborator gives every copy its own slot, and an owning one is
# destroyed within its own iteration.
struct Lanes[w: Int](Movable):
    var v: Int

    def __init__(out self, x: Int):
        self.v = x * Self.w

    def __deinit__(deinit self):
        print("del", Self.w)


def total[n: Int]() -> Int:
    var total = 0
    comptime for i in range(1, n):
        var v = SIMD[DType.int32, i](7)
        total += Int(v.reduce_add())
    return total


def bumped[T: AnyType, n: Int]() -> Int:
    var total = 0
    comptime for i in range(1, n):
        var v = SIMD[DType.int32, i](7)
        v += 1
        total += Int(v.reduce_add())
    return total


def show[n: Int]():
    comptime for i in range(1, n):
        var v = SIMD[DType.int32, i](7)
        print(v)


def nested[n: Int]() -> Int:
    var total = 0
    comptime for i in range(1, n):
        comptime for j in range(1, i + 1):
            var w = SIMD[DType.int32, j](i)
            total += Int(w.reduce_add())
    return total


def owned[n: Int]():
    comptime for i in range(1, n):
        var l = Lanes[i](3)
        print(l.v)
    print("end")


def main():
    print(total[3]())
    print(total[1]())
    print(bumped[Int, 3]())
    show[3]()
    print(nested[3]())
    owned[3]()
