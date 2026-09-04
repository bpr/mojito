# A Variant whose alternatives are all Hashable/Equatable/Writable satisfies
# those bounds nominally (generic `[T: ...]` parameters) and serves as a Dict
# key: the hasher sees the discriminant before the active payload, so equal
# payloads under different alternatives hash apart.
from std.utils import Variant

def show[T: Hashable & Equatable & Writable](x: T):
    print(hash(x) == hash(x), x == x)

def main() raises:
    var a = Variant[Int, String](7)
    var b = Variant[Int, String](7)
    var c = Variant[Int, String](String("7"))
    print(hash(a) == hash(b), hash(a) == hash(c), a == b, a == c)
    show(a)
    show(c)
    var table = Dict[Variant[Int, String], Int]()
    table[a] = 1
    table[c] = 2
    table[b] = 3
    print(len(table), table[Variant[Int, String](7)], table[c])
