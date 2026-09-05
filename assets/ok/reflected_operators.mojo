# Reflected operator dunders: when the left operand has no method for the
# pair, the right operand's `__radd__`/`__rmul__`/`__rsub__` answers.
@fieldwise_init
struct Meters(Copyable, Movable):
    var v: Int
    def __add__(self, other: Int) -> Int:
        return self.v + other
    def __radd__(self, other: Int) -> Int:
        return other + self.v + 100
    def __rmul__(self, other: Int) -> Int:
        return other * self.v
    def __rsub__(self, other: Int) -> Int:
        return other - self.v
def main():
    var m = Meters(3)
    print(m + 1)
    print(1 + m)
    print(2 * m)
    print(10 - m)
