# A struct's callable-value parameter is a compile-time value: it is part
# of the instance's identity, and its methods read and call it as
# `Self.f`, static or not, directly, through a local, and from a closure.


def seven() -> Int:
    return 7


def eight() -> Int:
    return 8


struct S[f: def() thin -> Int](Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x + Self.f()

    @staticmethod
    def sget() -> Int:
        var h = Self.f
        return h()

    def via_closure(self) -> Int:
        var y = self.x

        def body() {var y} -> Int:
            return y + Self.f()

        return body()


def main():
    var s = S[seven](1)
    var t = S[eight](1)
    print(s.x)
    print(s.get(), t.get())
    print(S[seven].sget(), S[eight].sget())
    print(t.via_closure())
