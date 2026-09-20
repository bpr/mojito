# A per-instantiation method clone inherits its checked template's facts when
# the body hands a place to a `mut` or bare `ref` parameter of a method call
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `place_arguments`): a local, a parameter, or a field of `self`. The callee's
# declared convention decides that the call keeps the caller's place, and the
# generations a `mut` argument invalidates lie below the argument's own
# binding, so an instance gets its own bindings back. A parameter of the
# struct's parameter type is admitted on a call of `self`'s own method, whose
# binders are the caller's.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T
    var counts: List[Int]
    var count: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.counts = List[Int]()
        self.counts.append(4)
        self.counts.append(5)
        self.count = 2

    def fill(self, mut into: Int):
        into += self.count

    def tally(self) -> Int:
        var n = 0
        self.fill(n)
        return n

    def look(self, ref other: Self.T) -> Self.T:
        return other

    def relook(self, ref other: Self.T) -> Self.T:
        return self.look(other)

    def own(self) -> Self.T:
        return self.look(self.item)

    def held(self, var value: Self.T) -> Self.T:
        var kept = value^
        return self.look(kept)

    def into(self, ref a: Int, mut n: Int):
        n += a

    def total(self) -> Int:
        var n = 5
        self.into(self.count, n)
        return n

    def size_at(self, i: Int) -> Int:
        return self.counts[i]

    def exchange(mut self, i: Int, mut n: Int):
        var old = self.size_at(i)
        self.counts[i] = n
        n = old

    def swap_in(mut self, i: Int, mut n: Int):
        self.exchange(i, n)


def main():
    var numbers = Shelf[Int](9)
    var n = 3
    numbers.swap_in(1, n)
    var three = 3
    print(n, numbers.tally(), numbers.relook(three), numbers.own())
    print(numbers.held(8), numbers.total(), numbers.size_at(1))
    var words = Shelf[String]("i")
    var w = String("w")
    words.swap_in(0, n)
    print(n, words.tally(), words.relook(w), words.own())
    print(words.held("h"), words.total(), words.size_at(0))
