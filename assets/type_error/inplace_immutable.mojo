# expect: must be mutable
# `__iadd__(mut self, …)` needs a mutable receiver. A borrowed (`read`) parameter
# is immutable, so augmenting it in place is rejected.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

def bump(c: Counter):
    c += 1

def main():
    var c = Counter(0)
    bump(c)
    print(c.value)
