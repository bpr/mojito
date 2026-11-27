# A view over a read parameter's origin is immutable (`View[False, …]`),
# so it does not convert to a mutable view of the same place.
# expect: cannot implicitly convert 'View[false, origin_of(xs)]' value to 'View[true, origin_of(xs)]'
struct View[view_mut: Bool, //, view_origin: Origin[mut=view_mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


def viewr(xs: List[Int]) -> View[origin_of(xs)]:
    return View[origin_of(xs)](len(xs))


def main():
    var xs: List[Int] = [1, 2]
    var w: View[origin_of(xs)] = viewr(xs)
    print(w.n)
