# A trait requirement's default may read the method's own value parameters:
# a call through the bound runs the requirement's default with the call's
# compile-time arguments in their place, whatever the witness declares, and
# a nominal call runs the witness's own default.

comptime TWO = 2


trait Scaler:
    def scale[n: Int](self, value: Int, factor: Int = n) -> Int:
        ...

    def shift[n: Int, m: Int](self, value: Int, by: Int = n * TWO - m) -> Int:
        ...

    def grow[step: Int = 4](self, value: Int, by: Int = step + 1) -> Int:
        ...


struct Doubler(Scaler):
    def __init__(out self):
        pass

    def scale[n: Int](self, value: Int, factor: Int = 7) -> Int:
        return value * factor

    def shift[n: Int, m: Int](self, value: Int, by: Int) -> Int:
        return value + by

    def grow[step: Int = 4](self, value: Int, by: Int = 100) -> Int:
        return value + by


struct Tripler(Scaler):
    def __init__(out self):
        pass

    def scale[n: Int](self, value: Int, factor: Int) -> Int:
        return value * factor * 3

    def shift[n: Int, m: Int](self, value: Int, by: Int = 0) -> Int:
        return value - by

    def grow[step: Int = 4](self, value: Int, by: Int = 0) -> Int:
        return value * by


def run[T: Scaler](s: T):
    var n = 100
    print(s.scale[2](5))
    print(s.scale[4](5) + n)
    print(s.scale[4](5, 1))
    print(s.shift[3, m=1](10))
    print(s.shift[m=4, n=2](10, by=0))
    print(s.grow(10))
    print(s.grow[1](10))


def main():
    run(Doubler())
    run(Tripler())
    print(Doubler().scale[2](5))
    print(Tripler().shift[3, 1](10))
    print(Doubler().grow(10))
