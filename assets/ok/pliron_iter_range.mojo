# Native iterator protocol over the scalar ranges: the one-, two-, and
# three-argument forms, an empty descending-bounds range, and nested loops.
# Every advance runs the raising `__next__` over the tagged-outcome ABI, so
# each header exercises the statically typed StopIteration edge.
def main():
    var total: Int = 0
    for i in range(4):
        total = total + i
    for j in range(2, 5):
        total = total + j
    for k in range(9, 1, -3):
        total = total + k
    for e in range(7, 3):
        total = total + 1000
    print(total)
    for a in range(2):
        for b in range(3):
            print(a * 10 + b)
