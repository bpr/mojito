# PROBE (rejection of valid Mojo): forwarding a named accessor's reference
# result as the method's own.
#
# `List.unsafe_get` returns a reference into the list's element interior, as
# `List.__getitem__` does. Mojito accepts the subscript forwarded under that
# origin and rejects the named call: only the subscript path records the
# interior generation the return check compares (`indexing.rs`,
# `record_interior_reference`).
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    3
#   mojito: returned reference escapes storage outside its declared origin
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def raw(
        ref self, index: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.items.unsafe_get(index)


def main():
    var shelf = Shelf[Int]()
    shelf.add(3)
    print(shelf.raw(0))
