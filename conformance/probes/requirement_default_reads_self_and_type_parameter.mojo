# Does a call through a bound run a requirement default reading `Self` or
# one of the method's type parameters?
#
# The pin prints `1514`: `x.scale(2)` takes the requirement's `Self.K`
# (7) and `x.scale2(B(), 3)` its `T.K` (5), not the witness's `10`.
# Mojito matches.
trait HasK:
    comptime K: Int

trait Scaler(HasK):
    def scale(self, value: Int, factor: Int = Self.K) -> Int:
        ...
    def scale2[T: HasK](self, t: T, value: Int, factor: Int = T.K) -> Int:
        ...

@fieldwise_init
struct A(Scaler):
    comptime K = 7
    var v: Int
    def scale(self, value: Int, factor: Int = 10) -> Int:
        return value * factor
    def scale2[T: HasK](self, t: T, value: Int, factor: Int = 10) -> Int:
        return value * factor

@fieldwise_init
struct B(HasK):
    comptime K = 5

def go[T: Scaler](x: T) -> Int:
    return x.scale(2) + x.scale2(B(), 3) * 100

def main():
    print(go(A(1)))
