# A `@staticmethod` of a value-parameterized struct reading its struct's
# parameter (`Self.k`) fails at run time with "field access on non-struct
# None": the erased static body reads `Self.k` off a `self` it does not have.
# The pinned Mojo prints 5 then 5. A call in such a body inferring a value
# parameter from `Counter[Self.k]` fails MIR verification for the same reason.
# Roadmap section 3 tracks it; promote this probe to `assets/ok` once it runs.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i


def size[n: Int](c: Counter[n]) -> Int:
    return n


struct W[k: Int]:
    @staticmethod
    def st() -> Int:
        return Self.k

    @staticmethod
    def sized() -> Int:
        return size(Counter[Self.k](1))


def main():
    print(W[5].st())
    print(W[5].sized())
