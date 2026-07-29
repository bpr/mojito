# expect: __iadd__
# The in-place dunder participates in ordinary argument checking: a right-hand
# side whose type does not match the `__iadd__` parameter is rejected.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

def main():
    var c = Counter(0)
    c += "not an int"
    print(c.value)
