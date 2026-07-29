# A value-returning subscript getter dispatches the element's in-place dunder:
# `r[i] += v` materializes the element, applies `__iadd__`, and writes the result
# back through `__setitem__` (Mojo does not use `__add__`).
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter
    var b: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a if i == 0 else self.b

    def __setitem__(mut self, i: Int, v: Counter):
        if i == 0:
            self.a = v
        else:
            self.b = v

def main():
    var r = Row(Counter(1), Counter(2))
    r[0] += 40
    r[1] += 5
    print(r[0].value, r[1].value)
