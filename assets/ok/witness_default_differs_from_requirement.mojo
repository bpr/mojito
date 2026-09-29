# A witness may default a parameter differently from its trait requirement,
# or not at all. A call through the bound runs the requirement's default,
# which the checker spells at the call once a conformer's witness declares it
# otherwise (`checker/bound_defaults.rs`); a call on a nominal receiver runs
# the witness's own.
trait Scaler:
    def scale(self, value: Int, factor: Int = 2) -> Int:
        ...

    def shift(self, value: Int, by: Int = 3, extra: Int = 100) -> Int:
        ...


struct Twice(Copyable, Movable, Scaler):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def scale(self, value: Int, factor: Int = 7) -> Int:
        return self.base + value * factor

    def shift(self, value: Int, by: Int, extra: Int = 1000) -> Int:
        return value + by + extra


struct Plain(Copyable, Movable, Scaler):
    def __init__(out self):
        pass

    def scale(self, value: Int, factor: Int) -> Int:
        return value * factor

    def shift(self, value: Int, by: Int = 9, extra: Int = 0) -> Int:
        return value - by - extra


struct Holder[S: Scaler & Copyable & Deinitable](Movable):
    var s: Self.S

    def __init__(out self, var s: Self.S):
        self.s = s^

    def run(self) -> Int:
        return self.s.scale(5)

    def run_keyword(self) -> Int:
        return self.s.scale(value=5)

    def run_explicit(self) -> Int:
        return self.s.scale(5, factor=10)

    def run_shift(self) -> Int:
        return self.s.shift(1) + self.s.shift(1, extra=0)


def through[S: Scaler](s: S) -> Int:
    return s.scale(4) + s.shift(0, 1)


def main():
    var h = Holder[Twice](Twice(1))
    print(h.run())
    print(h.run_keyword())
    print(h.run_explicit())
    print(h.run_shift())
    var p = Holder[Plain](Plain())
    print(p.run())
    print(p.run_shift())
    print(through(Twice(1)))
    print(through(Plain()))
    print(Twice(1).scale(5))
    print(Plain().shift(20))
    print(Twice(0).shift(1, 2))
