# Borrowed Set iteration through the generic reference-yielding protocol: the
# delegated `_ListIter` borrows the backing list through the seam handle two
# frames deep, and `for ref` writes through into the set's storage.
from std.collections.set import Set

def main():
    var s: Set[Int] = Set[Int]()
    s.add(1)
    s.add(2)
    for ref x in s:
        x += 10
    print(len(s))
    print(11 in s, 12 in s)
    var total = 0
    for y in s:
        total += y
    print(total)
