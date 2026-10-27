# A subscript store needs a getter: upstream's `emitGetterSetterAccess`
# types the stored value through `__getitem__`'s result, so a struct that
# declares `__setitem__` alone is rejected at the store.
# expect: has '__setitem__' but no '__getitem__'
struct Sink:
    var n: Int

    def __init__(out self):
        self.n = 0

    def __setitem__(mut self, i: Int, value: Int):
        self.n = value


def main():
    var s = Sink()
    s[0] = 3
    print(s.n)
