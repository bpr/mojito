# expect: requires an in-place method
# A subscript element that defines only `__add__` is rejected: augmented
# assignment dispatches the element's dedicated in-place dunder, with no fallback.
@fieldwise_init
struct Only(Copyable, Movable):
    var value: Int

    def __add__(self, o: Only) -> Only:
        return Only(self.value + o.value)

@fieldwise_init
struct Row:
    var a: Only

    def __getitem__(self, i: Int) -> Only:
        return self.a

    def __setitem__(mut self, i: Int, v: Only):
        self.a = v

def main():
    var r = Row(Only(1))
    r[0] += Only(2)
    print(r.a.value)
