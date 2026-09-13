# A callable struct whose `__call__` takes `mut self` and writes back through
# the receiver place: Mojito accepts it as a `def(...)` conformer, while the
# pin's conformance wants a read receiver (`def(Self, …) capturing thin`).
@fieldwise_init
struct Counter(def(Int) -> Int):
    var total: Int

    def __call__(mut self, amount: Int) -> Int:
        self.total += amount
        return self.total

def main():
    var count = Counter(10)
    print(count(5), count(2), count.total)
