# A pack-keyed `def` whose `comptime for` binds an element — an argument
# (`var first = args[i]`) or a default construction (`var value = Ts[i]()`)
# — is served by its template: each unrolled copy keeps its own local at
# its element's type. `Ts[i]()` constructs the element its index selects,
# a literal index included, under the pack's bound or a `where` clause; a
# `Defaultable` pack keeps no storage in an instance (`Ts.length`); and a
# `Tuple` bound to a type parameter or a pack element is default-built.
def show[*Ts: Writable & Copyable & ImplicitlyCopyable & Deinitable](*args: *Ts):
    comptime for i in range(len(Ts)):
        var first = args[i]
        print(first)


def reassigned[*Ts: Movable & Defaultable & Writable & Deinitable]():
    comptime for i in range(Ts.length):
        var value = Ts[i]()
        print(value)
        value = Ts[i]()
        print(value)


def nested[*Ts: Movable & Defaultable & Writable & Deinitable]():
    comptime for i in range(len(Ts)):
        comptime for j in range(2):
            var value = Ts[i]()
            print(value, j)


def bound[*Ts: Movable & Writable & Deinitable]() where conforms_to(
    Ts.values, Defaultable
):
    comptime for i in range(len(Ts)):
        var value = Ts[i]()
        print(value)


def ends[*Ts: Movable & Defaultable & Writable & Deinitable]():
    print(Ts[0](), Ts[1]())


def count[*Ts: Defaultable]() -> Int:
    return Ts.length


def one[T: Defaultable & Writable & Movable & Deinitable]():
    var value = T()
    print(value)


def main():
    show(1, "two", 3.5)
    reassigned[Int, Bool]()
    nested[Int, Bool]()
    bound[Int, Optional[Int], Tuple[Int, Bool]]()
    ends[String, Float64]()
    print(count[Int, Bool]())
    one[Tuple[Int, Bool]]()
