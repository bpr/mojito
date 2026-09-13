# Nominal callable structs devirtualize to direct `__call__` calls during
# backend monomorphization: a read-`self` `__call__` reads its own fields, a
# raising `__call__` reports through the tagged outcome and is caught, and an
# owned argument transfers through the invocation. A `mut self` `__call__`
# writing back through its receiver place is Mojito-only — upstream's
# `def(...)` conformance wants a read receiver — see the
# `mut-self-callable-struct` conformance case.
@fieldwise_init
struct Counter(def(Int) -> Int):
    var total: Int

    def __call__(self, amount: Int) -> Int:
        return self.total + amount

@fieldwise_init
struct Checked(def(Int) raises -> Int):
    var limit: Int

    def __call__(self, value: Int) raises -> Int:
        if value > self.limit:
            raise Error("over limit")
        return value * 2

@fieldwise_init
struct Keeper(def(mut List[Int], var String)):
    var seen: Int

    def __call__(self, mut sink: List[Int], var tag: String):
        sink.append(self.seen + tag.byte_length())

def main():
    var count = Counter(10)
    print(count(5))
    print(count(2))
    print(count.total)

    var checked = Checked(10)
    try:
        print(checked(4))
        print(checked(11))
    except e:
        print("caught")

    var keeper = Keeper(1)
    var sink: List[Int] = List[Int]()
    keeper(sink, String("abc"))
    keeper(sink, String("de"))
    print(sink[0], sink[1])
