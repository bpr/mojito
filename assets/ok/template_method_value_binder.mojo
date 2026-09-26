# A method keyed on a scalar value binder of its own, with no compile-time
# control flow, derives each per-call clone from the template the abstract
# check inferred: the elaborator folds the value to each call's literal,
# which keeps the name's identity and takes a literal's facts (read,
# accumulated, lent to a read parameter, or folded with other literals into
# one), on a generic struct and a plain one alike. A per-instantiation clone
# (`Box.scaled$y3:Int`) keeps the binder, bound to its own parameter.
def bump(x: Int) -> Int:
    return x + 1


struct Box[T: Copyable & Deinitable](Movable):
    var value: Self.T
    var count: Int

    def __init__(out self, var value: Self.T, count: Int):
        self.value = value^
        self.count = count

    def scaled[n: Int](self) -> Int:
        return self.count * n

    def mixed[n: Int](self, base: Int) -> Int:
        var acc = base * n
        acc += bump(n)
        acc += n * 10 + 1
        return acc + self.count

    def flagged[on: Bool](self) -> Bool:
        var seen = on
        return seen


struct Plain:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def scaled[n: Int](self) -> Int:
        return self.x * n


def main():
    var b = Box[Int](3, 4)
    var s = Box[String](String("v"), 2)
    print(b.scaled[3](), s.scaled[2](), b.scaled[2]())
    print(b.mixed[3](5), s.mixed[1](2))
    print(b.flagged[True](), s.flagged[False]())
    var p = Plain(7)
    print(p.scaled[3](), p.scaled[5]())
