# A display argument solves an inferred element parameter from its own
# elements, as at the pin: `f(["a", "b"], None)` binds `K` in `List[K]` to
# `String`, on a free call, a constructor, a fieldwise constructor, a
# method's own parameter, a `*xs: List[K]` collector, and against
# `Dict[K, V]` and `Set[T]`.
from std.collections import Set

struct W[K: Copyable & Deinitable]:
    var keys: List[Self.K]

    def __init__(out self, var keys: List[Self.K], n: Optional[Int]):
        self.keys = keys^


@fieldwise_init
struct F[K: Copyable & Deinitable]:
    var keys: List[Self.K]


struct S:
    def __init__(out self):
        pass

    def count[K: Copyable & Deinitable](self, var keys: List[K]) -> Int:
        return len(keys)


def f[K: Copyable & Deinitable](var keys: List[K], n: Optional[Int]) -> Int:
    return len(keys)


def collected[K: Copyable & Deinitable](*xs: List[K]) -> Int:
    return len(xs[0]) + len(xs[1])


def size[K: Hashable & Equatable & Copyable & Deinitable, V: Copyable & Deinitable](d: Dict[K, V]) -> Int:
    return len(d)


def members[T: Hashable & Equatable & Copyable & Deinitable](s: Set[T]) -> Int:
    return len(s)


def main():
    print(f(["a", "b"], None))
    var w = W(["a", "b"], None)
    print(len(w.keys), w.keys[0])
    var g = F([1.5, 2.5, 3.5])
    print(len(g.keys), g.keys[2])
    var s = S()
    print(s.count(["a", "b", "c"]))
    print(size({"a": 1, "b": 2}))
    print(members({1, 2, 3, 2}))
    print(collected(["a", "b"], ["c"]))
