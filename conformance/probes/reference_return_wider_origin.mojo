# PROBE (divergence): a returned reference declared at a wider origin than the
# place it names.
#
# The pinned Mojo compares the returned place's origin with the declared one
# exactly: `return self.item` needs `ref[origin_of(self.item)]`, and a List
# subscript needs the element interior of the field,
# `ref[origin_of(self.items)._get_owned_interior["element"]]`. Mojito asks
# only that the returned place lie within the declared origin
# (`origins/subst.rs:origin_is_within`), so it accepts the owner's origin for
# a field, and the field's origin for an element.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    error: cannot return reference with incompatible origin:
#           'origin_of(self.item)' vs 'origin_of(self)'
#           'origin_of(self.items["element"])' vs 'origin_of(self.items)'
#   mojito: 1, 2
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
