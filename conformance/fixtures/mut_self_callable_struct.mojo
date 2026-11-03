# A callable struct declaring a `def(...)` conformance with a `mut self`
# `__call__`: both compilers reject it, since the contract's method type
# takes a read receiver (`def(Self, ...) thin`).
@fieldwise_init
struct Counter(def(Int) -> Int):
    var total: Int

    def __call__(mut self, amount: Int) -> Int:
        self.total += amount
        return self.total

def main():
    var count = Counter(10)
    print(count(5), count(2), count.total)
