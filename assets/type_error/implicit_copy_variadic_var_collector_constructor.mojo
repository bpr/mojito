# expect: cannot be implicitly copied
# A constructor's owned `var *xs` collector copies a place it gathers, so the
# place's type must be `ImplicitlyCopyable`.
struct S:
    var n: Int

    def __init__(out self, var *xs: List[Int]):
        self.n = len(xs)


def main():
    var xs: List[Int] = [1, 2]
    var s = S(xs)
    print(s.n, len(xs))
