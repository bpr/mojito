# A per-instantiation method clone inherits its checked template's facts for
# a call to another struct's overloaded method on a place built over the
# struct parameter (`docs/notes/instantiation-from-template.md`): the callee's
# declared signature is read at the receiver's arguments before the caller's
# substitution, so each member of `List.extend` (`var other: Self` and
# `Span[Self.T, _]`) names its clone.


@fieldwise_init
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def double(mut self):
        self.items.extend(self.items.copy())

    def doubled(self) -> Int:
        var local = self.items.copy()
        local.extend(self.items.copy())
        return len(local)

    def absorb(mut self, other: List[Self.T]):
        self.items.extend(Span(other))


def main():
    var ints = Bag[Int](List[Int]())
    ints.items.append(1)
    ints.items.append(2)
    ints.double()
    print(len(ints.items), ints.doubled(), ints.items[3])
    var more = List[Int]()
    more.append(5)
    ints.absorb(more)
    print(len(ints.items), ints.items[4])
    var strings = Bag[String](List[String]())
    strings.items.append("a")
    strings.double()
    print(len(strings.items), strings.doubled(), strings.items[1])
    strings.absorb(strings.items.copy())
    print(len(strings.items))
