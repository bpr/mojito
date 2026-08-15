# Displacement-returning insert: Dict returns the displaced entry (key and
# value moved out); Set returns the displaced element.
from std.collections.set import Set

def main():
    var d: Dict[Int, Int] = {1: 10}
    var displaced = d.insert(1, 11)
    print(displaced.value().key, displaced.value().value)
    var s: Set[Int] = {7}
    print(s.insert(7).or_else(-1))
