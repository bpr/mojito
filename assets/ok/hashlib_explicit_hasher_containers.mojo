# `Dict[K, V, H]` and `Set[T, H]` take an explicit `Hasher` parameter and hash
# their keys with it; `hash[H](x)` selects a hasher the same way.
from std.collections.set import Set
from std.hashlib import default_comp_time_hasher

def main() raises:
    var d = Dict[String, Int, default_comp_time_hasher]()
    d["a"] = 1
    d["b"] = 2
    print(d["a"], d["b"], len(d))
    var s = Set[Int, default_comp_time_hasher]()
    s.add(3)
    print(3 in s, len(s))
    print(hash[default_comp_time_hasher](Int(1)))
    print(hash[default_comp_time_hasher](Int(1)) == hash[default_comp_time_hasher](Int(1)))
