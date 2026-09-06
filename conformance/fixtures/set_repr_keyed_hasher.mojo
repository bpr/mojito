# `repr(set)` spells the hasher parameter as upstream does: the keyed
# `AHasher[[0, 0, 0, 0] : SIMD[DType.uint64, 4]]` for `default_hasher`, a bare
# `Fnv1a` for an explicit one. VM-pinned: `repr` of a container does not lower
# natively yet (roadmap, native-lane follow-ups).
from std.collections import Set
from std.hashlib import default_comp_time_hasher

def main():
    var s: Set[Int] = {1, 2, 3}
    print(repr(s))
    var f = Set[Int, default_comp_time_hasher]()
    f.add(1)
    print(repr(f))
