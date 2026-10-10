# A list display argument to an explicitly parameterized generic call is
# checked against its parameter's solved type, as at the pin: against
# `List[Self.K]` it constructs a `List`, not an `Array`, on the
# single-signature, overloaded, and fieldwise constructor paths and on a free
# generic call, and a literal element materializes to the explicit element.
struct W[K: Copyable & Deinitable]:
    var keys: List[Self.K]

    def __init__(out self, var keys: List[Self.K], n: Optional[Int]):
        self.keys = keys^

    def __init__(out self, n: Int):
        self.keys = List[Self.K]()


@fieldwise_init
struct F[K: Copyable & Deinitable]:
    var keys: List[Self.K]


def f[K: Copyable & Deinitable](var keys: List[K], n: Optional[Int]) -> Int:
    return len(keys)


def total[T: Copyable & Deinitable & Writable](var xs: List[T], var ys: List[T]) -> Int:
    print(xs[0], ys[0])
    return len(xs) + len(ys)


def main():
    var d = Dict[String, Int](["a", "b"], [1, 2], None)
    print(len(d), d.get("b").value())
    var w = W[String](["a", "b"], None)
    print(len(w.keys), w.keys[1])
    var v = W[Int]([1, 2, 3], None)
    print(len(v.keys))
    var e = W[Int](0)
    print(len(e.keys))
    var g = F[String](["x", "y"])
    print(len(g.keys), g.keys[0])
    print(f[String](["a", "b"], None))
    print(total[Float64]([1, 2], [1.5]))
