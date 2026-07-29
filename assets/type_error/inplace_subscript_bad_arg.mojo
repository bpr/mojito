# expect: __iadd__
# The element in-place dunder participates in ordinary argument checking: a
# right-hand side whose type does not match its parameter is rejected.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a

    def __setitem__(mut self, i: Int, v: Counter):
        self.a = v

def main():
    var r = Row(Counter(0))
    r[0] += "not an int"
    print(r.a.value)
