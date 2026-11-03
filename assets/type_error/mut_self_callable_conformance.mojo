# A `def(...)` conformance requires a `__call__` with a read receiver: the
# contract's method type is `def(Self, ...) thin`, which a `mut self`
# `__call__` does not match.
# expect: does not match the signature required by trait
@fieldwise_init
struct Counter(def(Int) -> Int):
    var total: Int

    def __call__(mut self, amount: Int) -> Int:
        self.total += amount
        return self.total

def run[F: def(Int) -> Int](mut f: F) -> Int:
    return f(1)

def main():
    var c = Counter(10)
    print(run(c))
