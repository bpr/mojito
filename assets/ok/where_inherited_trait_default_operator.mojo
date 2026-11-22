# A `where` clause compares struct values through an operator method the
# struct inherits from a trait default (`Equatable.__ne__`), spelled or with
# a leading-dot operand, as upstream's `Pointer.gather` compares
# `Self.address_space != .GENERIC`.
@fieldwise_init
struct K(Equatable, ImplicitlyCopyable, TrivialRegisterPassable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v


struct S[a: AddressSpace = .GENERIC]:
    def __init__(out self):
        pass

    def spelled(self) -> Int where Self.a != AddressSpace.GENERIC:
        return 1

    def leading_dot(self) -> Int where Self.a != .GENERIC:
        return 2

    def plain(self) -> Int:
        return 3


struct T[k: K]:
    def __init__(out self):
        pass

    def nonzero(self) -> Int where Self.k != K(0):
        return Self.k.v


def main():
    print(S().plain())
    print(S[AddressSpace.GLOBAL]().plain())
    print(K(1) != K(2))
