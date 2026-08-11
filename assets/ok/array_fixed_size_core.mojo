# Fixed-size Array: contextual display construction, by-reference indexing,
# augmented element writes, equality, containment, copy, and all three
# iteration forms (borrowed value, `for ref` write-through, owned).
def main():
    var a: Array[Int, 3] = [1, 2, 3]
    print(len(a), a[0], a[2])
    a[1] += 5
    print(a)
    var b = a.copy()
    print(a == b, a != b)
    b[0] += 1
    print(a == b)
    print(7 in b, 99 in b)
    var total = 0
    for x in a:
        total += x
    print(total)
    for ref r in a:
        r += 1
    print(a)
    var moved = 0
    for var x in a^:
        moved += x
    print(moved)
