# expect: borrowing a different place
# An explicit origin argument in a local's annotation is a demand on every
# assignment, not only the initializer: `Span[Int, origin_of(xs)]` rejects a
# later value that borrows a different list, as it rejects such an initializer.
def main():
    var xs: List[Int] = [1]
    var ys: List[Int] = [2]
    var v: Span[Int, origin_of(xs)] = xs
    v = ys
    print(v[0])
