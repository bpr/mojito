# expect: requires a surrounding 'try'
# A raising element in-place dunder is an ordinary raising call: it must be
# handled by a surrounding `try` or a `raises` function.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int) raises:
        if amount < 0:
            raise Error("negative")
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a

    def __setitem__(mut self, i: Int, v: Counter):
        self.a = v

def main():
    var r = Row(Counter(1))
    r[0] += 40
    print(r.a.value)
