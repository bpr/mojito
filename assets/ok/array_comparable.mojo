# `Array` conforms to `Comparable` when its element type does (upstream
# 2026-08): ordering is lexicographic — the first differing element decides,
# so [1, 5] < [2, 3].
def main():
    var a: Array[Int, 2] = [1, 5]
    var b: Array[Int, 2] = [2, 3]
    print(a < b)
    print(a <= b)
    print(a > b)
    print(b >= a)
