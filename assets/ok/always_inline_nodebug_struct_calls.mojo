# Calls to `@always_inline("nodebug")` methods of parametric structs, and
# to such structs' `nodebug` constructors, are inlined before elaboration,
# the owner's parameters bound to the receiver's or the constructed type's
# arguments. Each prints what the call computes either way.


struct Box[k: Int]:
    var value: Int

    @always_inline("nodebug")
    def __init__(out self, value: Int):
        self.value = value * Self.k

    @always_inline("nodebug")
    def get(self) -> Int:
        return self.value + Self.k

    @always_inline("nodebug")
    def scaled[j: Int](self) -> Int:
        return self.value * j + Self.k

    @always_inline("nodebug")
    def twice(self) -> Int:
        return self.get() + Box[Self.k](1).get()


struct Named[T: Writable & ImplicitlyCopyable & Deinitable]:
    var name: String
    var item: Self.T

    @always_inline("nodebug")
    def __init__(out self, var name: String, item: Self.T):
        self.name = name^
        self.item = item

    @always_inline("nodebug")
    def label(self) -> String:
        return self.name + "=" + String(self.item)


def generic[n: Int](x: Int) -> Int:
    var b = Box[n](x)
    return b.get() + b.scaled[2]()


def main():
    var total = 0
    for i in range(3):
        var b = Box[2](i)
        total += b.get()
    print(total)
    print(generic[3](4), generic[5](1))
    var b = Box[4](3)
    print(b.twice(), b.scaled[10]())
    for i in range(2):
        var n = Named[Int]("n" + String(i), i)
        print(n.label())
