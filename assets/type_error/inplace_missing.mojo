# expect: requires an in-place method
# Augmented assignment requires the dedicated in-place dunder. A type that only
# defines `__add__` is rejected: Mojo does not fall back to the binary operator.
@fieldwise_init
struct OnlyAdd(Copyable, Movable):
    var value: Int

    def __add__(self, other: OnlyAdd) -> OnlyAdd:
        return OnlyAdd(self.value + other.value)

def main():
    var a = OnlyAdd(1)
    a += OnlyAdd(2)
    print(a.value)
