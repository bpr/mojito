# A requirement default reading the method's value parameters runs with the
# call's binding of them through a bound: a caller's own binder, arithmetic
# over two of them with a module constant, and an enclosing struct's
# parameter.
comptime TWO = 2


trait Scaler:
    def scale[n: Int](self, value: Int, factor: Int = n) -> Int:
        ...

    def mix[n: Int, m: Int](self, value: Int, factor: Int = n * TWO - m) -> Int:
        ...


struct Doubler(Scaler):
    def __init__(out self):
        pass

    def scale[n: Int](self, value: Int, factor: Int = 7) -> Int:
        return value * factor

    def mix[n: Int, m: Int](self, value: Int, factor: Int = 7) -> Int:
        return value * factor


def run[T: Scaler, k: Int, j: Int](s: T):
    print(s.scale[k](5))
    print(s.mix[k, j](5))
    print(s.mix[k, 1](5))
    print(s.mix[-k, j](5))


@fieldwise_init
struct Holder[w: Int]:
    def go[T: Scaler](self, s: T):
        print(s.scale[Self.w](5))


def main():
    run[Doubler, 3, 4](Doubler())
    Holder[4]().go(Doubler())
    print(Doubler().scale[3](5))
