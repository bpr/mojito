# Augmented assignment on a user-defined value dispatches to its in-place dunder
# (`__iadd__`), mutating `mut self`. Here the target is a projected field place,
# so the mutation must commit back through the field.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Wrapper:
    var inner: Counter

def main():
    var w = Wrapper(Counter(10))
    w.inner += 5
    print(w.inner.value)
