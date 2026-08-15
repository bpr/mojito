# Consuming `for var … in s^` iteration over a Set (imported explicitly —
# required upstream; Mojito's prelude visibility is a recorded divergence).
from std.collections.set import Set

def main():
    var s: Set[Int] = {1, 2}
    for var element in s^:
        print("set", element)
