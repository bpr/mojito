# `Pointer.unsafe_bitcast[U]()` retypes a pointer's pointee and keeps its
# address and origin: upstream's `pop.pointer.bitcast`.


@fieldwise_init
struct Pair(ImplicitlyCopyable):
    var a: Int
    var b: Int


def show[T: Writable](p: Pointer[T, _]):
    print(p.unsafe_bitcast[T]()[])


def bump(p: MutPointer[Pair, _]) -> Int:
    var q = p.unsafe_bitcast[Pair]()
    q[].b += 3
    return q[].b


def main():
    var x: UInt8 = 200
    var p = Pointer(to=x)
    var q = p.unsafe_bitcast[Byte]()
    print(q[])
    q[] = 7
    var r = q.unsafe_bitcast[U=Scalar[DType.uint8]]()
    print(r[])
    show(r)

    var pair = Pair(1, 2)
    print(bump(Pointer(to=pair)))
    print(pair.a, pair.b)
