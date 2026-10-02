# expect: cannot be implicitly copied
# A method's owned `var *xs` collector copies a place it gathers, exactly as
# a free function's does, so the place's type must be `ImplicitlyCopyable`.
@fieldwise_init
struct S:
    var n: Int

    def take(self, var *xs: List[Int]) -> Int:
        return 1


def main():
    var xs: List[Int] = [1, 2]
    var s = S(1)
    print(s.take(xs))
    print(len(xs))
