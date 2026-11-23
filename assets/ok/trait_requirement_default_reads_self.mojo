# A requirement default reading `Self` (`Self.K`, `Self()`) or the method's
# own type parameter (`T.K`), run by a call through the bound that leaves it
# out, at a parameter of type `Int` and of type `Self`.
# Expected output: 1514, then 10
trait HasK:
    comptime K: Int

trait Scaler(HasK, Defaultable, Movable):
    def scale(self, value: Int, factor: Int = Self.K) -> Int:
        ...
    def scale2[T: HasK](self, t: T, value: Int, factor: Int = T.K) -> Int:
        ...
    def plus(self, other: Self = Self()) -> Int:
        ...

struct A(Scaler):
    comptime K = 7
    var v: Int

    def __init__(out self):
        self.v = 9

    def __init__(out self, v: Int):
        self.v = v

    def scale(self, value: Int, factor: Int = 10) -> Int:
        return value * factor

    def scale2[T: HasK](self, t: T, value: Int, factor: Int = 10) -> Int:
        return value * factor

    def plus(self, other: Self = Self(100)) -> Int:
        return self.v + other.v

@fieldwise_init
struct B(HasK):
    comptime K = 5

def go[T: Scaler](x: T) -> Int:
    return x.scale(2) + x.scale2(B(), 3) * 100

def add[T: Scaler](x: T) -> Int:
    return x.plus()

def main():
    print(go(A(1)))
    print(add(A(1)))
