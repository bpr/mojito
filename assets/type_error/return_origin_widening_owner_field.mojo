# expect: cannot return reference with incompatible origin
# A returned reference names exactly its declared origin, as at the pin: a
# field is a different origin from its owner, so `return self.item` needs
# `ref[origin_of(self.item)]`, and a List element needs the element interior
# of the field, `ref[origin_of(self.items)._get_owned_interior["element"]]`.
# Both owner-wide spellings below are rejected.
struct Slot[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.item = item.copy()
        self.items = List[Self.T]()
        self.items.append(item^)

    def peek(ref self) -> ref[origin_of(self)] Self.T:
        return self.item

    def at(ref self, index: Int) -> ref[origin_of(self.items)] Self.T:
        return self.items[index]


def main():
    var slot = Slot(1)
    print(slot.peek())
    print(slot.at(0) + 1)
