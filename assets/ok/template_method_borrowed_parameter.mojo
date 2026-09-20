# A per-instantiation method clone inherits its checked template's facts when
# the method takes a `mut` or a bare `ref` parameter (`docs/notes/
# instantiation-from-template.md`, class MethodBody). Such a parameter is bound
# from its declared convention and rooted at its own binding under every
# instance, so the body's facts name it by template owner. A `mut` parameter
# may be stored to; a bare `ref` one has parametric mutability and is only
# read. What the caller owes lives in the signature, checked per clone.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T
    var count: Int

    def __init__(out self, var item: Self.T, count: Int):
        self.item = item^
        self.count = count

    def tally(self, mut into: Int):
        into += self.count

    def reset(self, mut into: Int):
        into = 0

    def put(self, mut slot: Self.T):
        slot = self.item

    def same(self, ref other: Self.T) -> Self.T:
        return other

    def larger(self, ref other: Int) -> Bool:
        return other > self.count


def main():
    var numbers = Shelf[Int](7, 2)
    var words = Shelf[String]("held", 3)
    var total = 0
    numbers.tally(total)
    words.tally(total)
    print(total)
    var n = 1
    var text = String("mine")
    print(numbers.same(n), words.same(text))
    numbers.put(n)
    words.put(text)
    print(n, text)
    print(numbers.larger(total), words.larger(n))
    numbers.reset(total)
    print(total)
