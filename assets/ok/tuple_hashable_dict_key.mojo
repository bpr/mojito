# Tuple is Hashable when every element is: tuples serve as Dict keys and Set
# elements through the hasher protocol (each element feeds the hasher in
# order), and a generic `[T: Hashable]` bound accepts them. Hash values are
# only compared for equality (the bundled hasher is seedable).
from std.collections.set import Set

def key_hash[T: Hashable](x: T) -> UInt64:
    return hash(x)

def main() raises:
    var grid = Dict[Tuple[Int, Int], String]()
    grid[(0, 0)] = "origin"
    grid[(1, 2)] = "east"
    grid[(1, 2)] = "east-again"
    print(len(grid))
    print(grid[(1, 2)])
    print((0, 0) in grid)
    print((2, 1) in grid)

    var seen = Set[Tuple[Int, String]]()
    seen.add((1, "a"))
    seen.add((1, "a"))
    seen.add((2, "b"))
    print(len(seen))
    print((2, "b") in seen)

    print(hash((1, 2)) == hash((1, 2)))
    print(key_hash((3, "x")) == hash((3, "x")))
    print(len((1, 2, 3)))
