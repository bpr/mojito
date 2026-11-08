# A static read through a bare parametric struct binds none of the struct's
# parameters, defaulted or not, so it is a parametric function, which is no
# runtime value.
# expect: cannot use parametric function as a runtime closure
struct D[T: Copyable = Int]:
    @staticmethod
    def st(x: Int) -> Int:
        return x + 2


def main():
    var d = D.st
    print(d(4))
