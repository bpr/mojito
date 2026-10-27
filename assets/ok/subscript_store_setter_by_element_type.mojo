# A subscript store converts its value to the getter's result type before it
# selects the setter, as upstream's `SubscriptDLValue` stores a value of its
# element type: an `Int` right-hand side reaches the `W` setter through `W`'s
# implicit constructor, not the exact `Int` overload.
struct W(Copyable, Movable):
    var x: Int

    @implicit
    def __init__(out self, x: Int):
        self.x = x


struct S:
    var n: Int

    def __init__(out self):
        self.n = 0

    def __getitem__(self, i: Int) -> W:
        return W(self.n)

    def __setitem__(mut self, i: Int, value: W):
        print("w")
        self.n = value.x

    def __setitem__(mut self, i: Int, value: Int):
        print("int")
        self.n = value


def main():
    var s = S()
    var k: Int = 3
    s[0] = k
    print(s.n)
