# A binary operator method takes exactly two operands, its receiver
# included: a defaulted third parameter is rejected at the declaration,
# in a trait requirement as in a struct method.
# expect: '__add__' requires 2 operands
trait Idx:
    def __add__(self, other: Int, k: Int = 50) -> Int:
        ...


@fieldwise_init
struct S(Copyable, Idx):
    var v: Int

    def __add__(self, other: Int, k: Int = 50) -> Int:
        return self.v + other + k


def bump[T: Idx](x: T) -> Int:
    return x + 1


def main():
    print(bump(S(0)))
