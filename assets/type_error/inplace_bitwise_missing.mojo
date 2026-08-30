# expect: requires an in-place method
# The ordinary binary dunder is not a fallback for augmented assignment.
@fieldwise_init
struct OnlyOr(Copyable, Movable):
    var value: Int

    def __or__(self, other: Self) -> Self:
        return OnlyOr(self.value | other.value)

def main():
    var value = OnlyOr(1)
    value |= OnlyOr(2)
