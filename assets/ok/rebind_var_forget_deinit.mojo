# Upstream's library `rebind_var` moves an owned value into the type a
# parameter refinement names, and `std.memory.forget_deinit` ends a value
# without running its deinitializer or its fields'. Both take a `var`
# argument of an unbounded parameter type: a transfer into an owned argument
# is forwarded in place and needs no `Movable`.
from std.memory import forget_deinit


@fieldwise_init
struct Child(Movable):
    var n: Int

    def __deinit__(deinit self):
        print("child deinit", self.n)


@fieldwise_init
struct Parent(Movable):
    var child: Child

    def __deinit__(deinit self):
        print("parent deinit")


struct Pinned(Movable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id


def take(var value: Pinned) -> Int:
    return value.id


def forward[T: AnyType](var value: T):
    forget_deinit(value^)


def identity[T: Movable](var value: T) -> T:
    return rebind_var[T](value^)


struct Bag[T: Movable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def put[U: Movable](mut self, var item: U):
        self.items.append(rebind_var[type_of(self).T](item^))


def main():
    forget_deinit(Child(1))
    var parent = Parent(Child(2))
    forget_deinit(parent^)
    forward(Child(3))
    var kept = identity(Child(4))
    print("kept", kept.n)
    var p = Pinned(5)
    print(take(p^))
    var bag = Bag[String]()
    bag.put(String("x"))
    bag.put(String("y"))
    print(len(bag.items), bag.items[0], bag.items[1])
    print("end")
