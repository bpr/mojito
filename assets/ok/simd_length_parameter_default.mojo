# A `SIMDLength` parameter default in an `Int` slot binds its lane count, as
# an explicit `SIMDLength` argument there does: upstream's builtin
# `SIMD.__init__(SIMDLength)` folded. `SIMD._shuffle_variadic`'s
# `output_size: Int = Self.length` is this default.


def lanes[w: SIMDLength, n: Int = w]() -> Int:
    return n


struct Lanes[length: SIMDLength]:
    def __init__(out self):
        pass

    def count[n: Int = Self.length](self) -> Int:
        return n

    def total[*mask: SIMDLength, n: Int = Self.length](self) -> Int:
        return mask.size + n

    def forward[*mask: SIMDLength](self) -> Int:
        return self.total[*mask]()


def main():
    print(lanes[4]())
    print(lanes[4, 7]())
    print(Lanes[4]().count())
    print(Lanes[8]().count[2]())
    print(Lanes[4]().total[1, 2]())
    print(Lanes[4]().forward[1, 2]())
