# A method overloads on whether its compile-time parameter is a pack: a
# bracket of several values selects `pick[*ks: Int]`, a named one `pick[k:
# Bool]`, beside the same run-time parameters.


struct Picker:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def pick[*ks: Int](self) -> Int:
        var total = self.base
        comptime for i in range(ks.size):
            total += ks[i]
        return total

    def pick[k: Bool](self) -> Int:
        return self.base if k else -self.base

    def pick[*ks: Int](self, scale: Int) -> Int:
        return self.pick[*ks]() * scale

    def pick[k: Bool](self, scale: Int) -> Int:
        return self.pick[k=k]() * scale


def main():
    var p = Picker(10)
    print(p.pick[1, 2, 3]())
    print(p.pick[k=False]())
    print(p.pick[4](2))
    print(p.pick[k=True](3))
