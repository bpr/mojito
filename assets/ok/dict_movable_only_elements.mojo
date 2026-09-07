# Upstream's Movable-only element bounds: a `Dict` value and a `Set` element
# need only be `Movable` (keys `Hashable & Equatable & Movable`), with the
# copying APIs (`copy`, `get`, the borrowing views, the set algebra)
# available where the elements are `Copyable`.
from std.collections import Set
from std.hashlib import Hasher

struct NC(Movable, Deinitable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

struct NCKey(Hashable, Equatable, Movable, Deinitable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def __hash__(self, mut hasher: Some[Hasher]):
        hasher.update(self.k)

    def __eq__(self, other: Self) -> Bool:
        return self.k == other.k

def main() raises:
    var d = Dict[String, NC]()
    d["a"] = NC(7)
    print(len(d), d["a"].v, "a" in d, "b" in d)
    var taken = d.pop("a")
    print(taken.v, len(d))

    var s = Set[NCKey]()
    s.add(NCKey(1))
    s.add(NCKey(1))
    s.add(NCKey(2))
    print(len(s), NCKey(1) in s, NCKey(3) in s)
    s.remove(NCKey(1))
    print(len(s))
