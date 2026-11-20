# A tuple element holding a `Pointer` is addressed as that element's slot:
# `Tuple.reverse` and `Tuple.concat` write each element through
# `Pointer(to=result[i])`, which stores the pointer itself, never through
# the pointer the slot held.
def main():
    var left = 4
    var right = 8
    var r = (Pointer(to=left), 7).reverse()
    print(r[0], r[1][])
    var c = (1, Pointer(to=left)).concat((Pointer(to=right), 2))
    print(c[0], c[1][], c[2][], c[3])
    print(left, right)
