# A per-instantiation method clone inherits its checked template's facts when
# the method takes a `ref` parameter with an origin clause
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `origin_parameters`): the clause names one of the method's own origin
# binders. An origin binder is inferred at every call and erased before
# execution, so a clone keeps it and binds it symbolically as the template
# does. `bump` writes through a `MutOrigin`, which is judged
# per instantiation, so its clones are still checked.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T
    var count: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.count = 2

    def first[o: Origin](self, ref[o] x: Self.T) -> Self.T:
        return x

    def pick[o: Origin](self, ref[o] x: Self.T) -> ref[o] Self.T:
        return x

    def add[o: ImmOrigin](self, ref[o] n: Int) -> Int:
        return n + self.count

    def bump[o: MutOrigin](self, ref[o] n: Int):
        n += self.count


def main():
    var numbers = Shelf[Int](9)
    var seven = 7
    numbers.bump(seven)
    print(numbers.first(seven), numbers.add(seven))
    print(numbers.pick(seven))
    var words = Shelf[String]("i")
    var w = String("w")
    words.bump(seven)
    print(words.first(w), words.add(seven))
    print(words.pick(w))
    print(w)
