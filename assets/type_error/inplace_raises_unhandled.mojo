# expect: requires a surrounding 'try'
# A raising in-place dunder is an ordinary raising call: it must be handled by a
# surrounding `try` or a `raises` function, or it is rejected.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int) raises:
        if amount < 0:
            raise Error("negative")
        self.value += amount

def main():
    var c = Counter(0)
    c += 5
    print(c.value)
