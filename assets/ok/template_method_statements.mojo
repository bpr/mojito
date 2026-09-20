# A per-instantiation method clone inherits its checked template's facts when
# the body holds runtime statements over closed scalars (`docs/notes/
# instantiation-from-template.md`, class MethodBody, feature `statements`): a
# `mut` or `var` receiver, a static method, a `where` clause, scalar locals,
# scalar field writes, `if`, `while`, a bare `return`, and a discarded call.
@fieldwise_init
struct Tally[T: Copyable & Deinitable](Copyable):
    var item: Self.T
    var count: Int
    var limit: Int

    def size(self) -> Int:
        return self.count

    def bump(mut self):
        self.count += 1

    def reset(mut self, to: Int):
        if to < 0:
            self.count = 0
            return
        self.count = to

    def fill(mut self):
        while self.count < self.limit:
            self.bump_by(2)
            if self.count > 100:
                break

    def bump_by(mut self, step: Int):
        self.count += step

    def triangle(self, n: Int) -> Int:
        var total = 0
        var i = 1
        while i <= n:
            total += i
            i += 1
        return total

    def clamped(self) -> Int where conforms_to(Self.T, Copyable):
        _ = self.size()
        if self.count > self.limit:
            return self.limit
        elif self.count < 0:
            return 0
        else:
            return self.count

    def spend(var self) -> Int:
        self.count -= 1
        return self.count

    @staticmethod
    def width() -> Int:
        var bits = 8
        bits *= 8
        return bits


def main():
    var a = Tally(7, 3, 9)
    var b = Tally(String("x"), 50, 20)
    a.bump()
    b.bump()
    print(a.size(), b.size())
    a.reset(-4)
    b.reset(12)
    print(a.size(), b.size())
    a.fill()
    b.fill()
    print(a.size(), b.size())
    print(a.triangle(4), b.triangle(10))
    b.reset(40)
    print(a.clamped(), b.clamped())
    print(Tally[Int].width(), Tally[String].width())
    print(a.copy().spend(), b.copy().spend())
