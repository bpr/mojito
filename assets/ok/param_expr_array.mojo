# A symbolic `Array` length validates as written and closes under an explicit
# binding: `Array[Int, n + 1]` at `n = 3` is `Array[Int, 4]`.
def grow[n: Int](var x: Array[Int, n + 1]) -> Array[Int, 1 + n]:
    return x^


def main():
    var source: Array[Int, 4] = [1, 2, 3, 7]
    var a: Array[Int, 4] = grow[3](source^)
    print(a[3])
