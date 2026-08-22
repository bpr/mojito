# Nominal callable structs devirtualize to direct `__call__` calls during
# backend monomorphization: a mut-self counter writes back through its
# receiver place, a raising `__call__` reports through the tagged outcome
# and is caught, and an owned argument transfers through the invocation.
@fieldwise_init
struct Counter(def(Int) -> Int):
    var total: Int

    def __call__(mut self, amount: Int) -> Int:
        self.total += amount
        return self.total

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

    def __call__(mut self, mut sink: List[Int], var tag: String):
        self.seen += len(tag)
        sink.append(self.seen)

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

    var keeper = Keeper(0)
    var sink: List[Int] = List[Int]()
    keeper(sink, String("abc"))
    keeper(sink, String("de"))
    print(sink[0], sink[1])
