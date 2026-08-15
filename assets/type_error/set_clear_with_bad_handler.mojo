# expect: no overload matches
from std.collections.set import Set

def main():
    var s: Set[Int] = {1}
    s.clear_with(lambda (a: Int, b: Int): None)
