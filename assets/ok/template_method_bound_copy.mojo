# A per-instantiation method clone inherits its checked template's facts when
# the body copies a place of the struct's parameter type through its
# `Copyable` bound (`docs/notes/instantiation-from-template.md`, class
# MethodBody, feature `bound_dispatch`). The template records the abstract
# dispatch; an instance decides the copy on its own type: a built-in value's
# copy is the read of the place itself, and a struct's is its own `copy`.
# The copy is a temporary, so it may be handed on by value.
struct Bag[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.items = List[Self.T]()

    def dup(self) -> Self.T:
        return self.item.copy()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_dup(mut self):
        self.items.append(self.item.copy())

    def push_twice(mut self, var value: Self.T):
        self.push(value.copy())
        self.push(value^)

    def keep(mut self, value: Self.T):
        var mine = value.copy()
        self.item = mine^


def main():
    var a = Bag[Int](1)
    var b = Bag[String](String("x"))
    a.push_dup()
    a.push_twice(3)
    a.keep(7)
    b.push_dup()
    b.push_twice(String("z"))
    b.keep(String("k"))
    print(a.dup(), b.dup(), len(a.items), len(b.items))
    print(a.items[2], b.items[2])
