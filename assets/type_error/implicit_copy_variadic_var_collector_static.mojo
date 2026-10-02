# expect: cannot be implicitly copied
# A static method's owned `var *xs` collector copies a place it gathers, so
# the place's type must be `ImplicitlyCopyable`.
struct S:
    var n: Int

    @staticmethod
    def stake(var *xs: List[Int]) -> Int:
        return 2


def main():
    var xs: List[Int] = [1, 2]
    print(S.stake(xs))
    print(len(xs))
