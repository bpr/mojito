# A closed module constant is a parameter value at every read: a collection
# display is one compile-time application a lifted initializer reads whole,
# and a `DType`, vector, tuple, or struct constant is a folded attribute read
# in a body, a parameter default, a bracket argument, and an alias.


@fieldwise_init
struct P(ImplicitlyCopyable, Movable):
    var x: Int
    var y: Int


def total[n: Int](xs: Array[Int, n]) -> Int:
    var s = 0
    for i in range(n):
        s += xs[i]
    return s


def size(d: Dict[String, Int]) -> Int:
    return len(d)


def sum_list(xs: List[Int]) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s


comptime L = [1, 2, 3]
comptime M = {"a": 1, "b": 2}
comptime A: List[Int] = [4, 5]
comptime S = total(L)
comptime N = size(M)
comptime T = (1, 2)
comptime V = SIMD[DType.int32, 4](1, 2, 3, 4)
comptime D = DType.int16
comptime V4 = SIMD[D, 4]
comptime PS = (P(1, 2), P(3, 4))
comptime SA = sum_list(A)


def lane(v: SIMD[DType.int32, 4] = V) -> Int:
    return Int(v[2])


def show[v: SIMD[DType.int32, 4]]():
    print(v)


def name[d: DType = D]():
    print(d)


def main():
    print(S, N, SA)
    print(T[0], T[1], V, V[3])
    var t = T
    print(t[1], lane())
    show[V]()
    name()
    print(V4(7), Scalar[D](3))
    print(PS[1].y)
