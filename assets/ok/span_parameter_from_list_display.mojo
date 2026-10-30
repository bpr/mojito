# A list display bound to a compile-time `Span` parameter is its natural
# `Array`, converted by `Span`'s `@implicit` `ref` constructor, closed or
# symbolic in its elements.


def size[c: Span[Int, _]]() -> Int:
    comptime n = len(c)
    return n


def second[c: Span[Int, _]]() -> Int:
    comptime x = c[1]
    return x


def show[c: Span[Int, _]]():
    print(len(c), c[1])
    comptime for i in range(len(c)):
        print(c[i])


def total[dtype: DType, c: Span[Scalar[dtype], _]]() -> Scalar[dtype]:
    var sum: Scalar[dtype] = 0
    comptime for i in range(len(c)):
        sum += c[i]
    return sum


def generic_total[dtype: DType]() -> Scalar[dtype]:
    return total[dtype, [Scalar[dtype](1.5), 2.0, 3]]()


def main():
    print(size[[1, 2, 3]](), second[[4, 5, 6]]())
    show[[7, 8, 9]]()
    print(total[DType.float64, [Float64(0.5), 1.0]]())
    print(generic_total[DType.float32](), generic_total[DType.float64]())
