# A struct's associated compile-time value read as `Self.N` in its own method
# and in an inherited trait default holding a `comptime if`.
trait Sized2:
    comptime N: Int

    def size(self) -> Int:
        comptime if Self.N > 2:
            return Self.N * 10
        else:
            return Self.N


@fieldwise_init
struct A(Sized2):
    comptime N: Int = 3
    var x: Int

    def own(self) -> Int:
        return Self.N


@fieldwise_init
struct B(Sized2):
    comptime N: Int = 1
    var x: Int


def main():
    print(A(3).own())
    print(A(3).size())
    print(B(1).size())
