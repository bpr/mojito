# A function-local `comptime` float is read at run time as a `Float64`,
# whether it folds from a `Float64` construction, a literal, or arithmetic.
def show[T: Writable & Copyable](value: T):
    comptime quarter = 0.25
    print(value, quarter)


@fieldwise_init
struct Scale:
    var factor: Float64

    def scaled(self) -> Float64:
        comptime k = Float64(1.5)
        return self.factor * k


def main():
    comptime x = Float64(2.5)
    comptime third = 1.0 / 3.0
    comptime zero = Float64()
    comptime neg = -x
    print(x)
    print(third, zero)
    print(String(x), neg < 0.0)
    show(x)
    show(neg)
    print(Scale(2.0).scaled())
