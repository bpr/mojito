# A per-instantiation method clone inherits its checked template's facts when
# the body hands a value of a struct parameter's type by value through the
# parameter's bound to a requirement parameter typed `Self` or by the
# requirement's own binder (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `bound_dispatch`). The argument's type and the
# parameter's become the same type in every instance, so a borrowed place is
# read where it lies, and an owned argument is a temporary, a transferred
# local, or a place the template recorded copying, whatever the witness.
trait Merger:
    def merge(self, other: Self) -> Int:
        ...

    def absorb(self, var other: Self) -> Int:
        ...

    def pair[T: Writable & Copyable](self, other: T) -> String:
        ...


@fieldwise_init
struct Count(Copyable, Deinitable, ImplicitlyCopyable, Merger):
    var n: Int

    def merge(self, other: Self) -> Int:
        return self.n + other.n

    def absorb(self, var other: Self) -> Int:
        return self.n * other.n

    def pair[T: Writable & Copyable](self, other: T) -> String:
        return String(self.n) + "." + String(other)


@fieldwise_init
struct Word(Copyable, Deinitable, Merger):
    var text: String

    def merge(self, other: Self) -> Int:
        return self.text.byte_length() + other.text.byte_length()

    def absorb(self, var other: Self) -> Int:
        return (self.text + other.text).byte_length() * 2

    def pair[T: Writable & Copyable](self, other: T) -> String:
        return self.text + "-" + String(other)


struct Holder[S: Merger & Copyable & Deinitable, W: Writable & Copyable & Deinitable](Movable):
    var s: Self.S
    var t: Self.S
    var w: Self.W

    def __init__(out self, var s: Self.S, var t: Self.S, var w: Self.W):
        self.s = s^
        self.t = t^
        self.w = w^

    def merged_field(self) -> Int:
        return self.s.merge(self.t)

    def merged_parameter(self, other: Self.S) -> Int:
        return self.s.merge(other)

    def merged_local(self) -> Int:
        var other = self.t.copy()
        return self.s.merge(other)

    def absorbed_copy(self) -> Int:
        return self.s.absorb(self.t.copy())

    def absorbed_transfer(self) -> Int:
        var other = self.t.copy()
        return self.s.absorb(other^)

    def paired_field(self) -> String:
        return self.s.pair(self.w)

    def paired_parameter(self, other: Self.W) -> String:
        return self.s.pair(other)


struct Implicit[S: Merger & ImplicitlyCopyable & Deinitable](Movable):
    var s: Self.S
    var t: Self.S

    def __init__(out self, s: Self.S, t: Self.S):
        self.s = s
        self.t = t

    def absorbed_field(self) -> Int:
        return self.s.absorb(self.t)

    def absorbed_parameter(self, other: Self.S) -> Int:
        return self.s.absorb(other)


def main():
    var count = Holder[Count, String](Count(2), Count(5), String("w"))
    var word = Holder[Word, Int](Word(String("ab")), Word(String("cde")), 7)
    print(
        count.merged_field(),
        count.merged_parameter(Count(1)),
        count.merged_local(),
        count.absorbed_copy(),
        count.absorbed_transfer(),
        count.paired_field(),
        count.paired_parameter(String("q")),
    )
    print(
        word.merged_field(),
        word.merged_parameter(Word(String("x"))),
        word.merged_local(),
        word.absorbed_copy(),
        word.absorbed_transfer(),
        word.paired_field(),
        word.paired_parameter(9),
    )
    var implicit = Implicit[Count](Count(3), Count(4))
    print(implicit.absorbed_field(), implicit.absorbed_parameter(Count(6)))
