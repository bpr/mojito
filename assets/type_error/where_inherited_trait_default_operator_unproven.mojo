# expect: lacking evidence to prove correctness
# A `where` clause over an operator method the struct inherits from a trait
# default (`AddressSpace.__ne__`, `Equatable`'s) checks at its declaration,
# but no call proves it: the comparison runs an ordinary method, which no
# constraint solver reads, as the pin reports.
struct S[a: AddressSpace = .GENERIC]:
    def __init__(out self):
        pass

    def g(self) -> Int where Self.a != AddressSpace.GENERIC:
        return 1


def main():
    print(S().g())
