# A trait requirement's default may name a module constant: the call through
# the bound runs the value the name has in the trait's scope.

comptime TWO = 2
comptime SIX = TWO * 3
comptime GREETING = "hello"
comptime RATIO = 0.5


trait Scaler:
    def scale(self, value: Int, factor: Int = TWO) -> Int:
        ...

    def shift(self, value: Int, by: Int = -SIX + 1) -> Int:
        ...

    def greet(self, name: String = GREETING) -> String:
        ...

    def weigh(self, value: Float64, ratio: Float64 = RATIO) -> Float64:
        ...


struct Doubler(Scaler):
    def __init__(out self):
        pass

    def scale(self, value: Int, factor: Int = 3) -> Int:
        return value * factor

    def shift(self, value: Int, by: Int) -> Int:
        return value + by

    def greet(self, name: String = "bye") -> String:
        return "doubler " + name

    def weigh(self, value: Float64, ratio: Float64 = 0.25) -> Float64:
        return value * ratio


struct Plain(Scaler):
    def __init__(out self):
        pass

    def scale(self, value: Int, factor: Int = TWO) -> Int:
        return value + factor

    def shift(self, value: Int, by: Int = -5) -> Int:
        return value - by

    def greet(self, name: String = GREETING) -> String:
        return "plain " + name

    def weigh(self, value: Float64, ratio: Float64 = RATIO) -> Float64:
        return value + ratio


def run[T: Scaler](s: T):
    var local = 100
    print(s.scale(5) + local)
    print(s.shift(10))
    print(s.greet())
    print(s.weigh(4.0))


def main():
    run(Doubler())
    run(Plain())
    print(Doubler().scale(3))
    print(Doubler().greet())
    print(Plain().shift(1))
