# A trait default is its trait's member over the trait's `_Self` binder,
# which the receiver of each conformer's forwarding call binds: every
# conformer, generic or not, runs its own instance of the member.
trait Greet:
    def name(self) -> Int: ...

    def hello(self) -> Int:
        return self.name() + 1

    def twice(self, k: Int = 2) -> Int:
        return self.hello() * k


struct Box[T: ImplicitlyCopyable & Deinitable](Greet):
    var x: Int
    var v: Self.T

    def __init__(out self, x: Int, v: Self.T):
        self.x = x
        self.v = v

    def name(self) -> Int:
        return self.x


struct S(Greet):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def name(self) -> Int:
        return self.x


struct R(Greet):
    def __init__(out self):
        pass

    def name(self) -> Int:
        return 70


def call[G: Greet](g: G) -> Int:
    return g.hello() + g.twice()


def main():
    var b = Box[Int](41, 1)
    var c = Box[Float64](10, 1.0)
    print(b.hello(), c.hello(), S(5).hello(), R().hello(), b.twice(3))
    print(call(b), call(S(2)), call(R()))
