# expect: 'x' is borrowed mutably and also used at the same call
# A `RegisterPassable` value that is not `TrivialRegisterPassable` keeps its
# read argument overlapping a `mut` borrow of the same value, as at the pin.
struct P(ImplicitlyCopyable, RegisterPassable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __imul__(mut self, rhs: Self):
        self.v = self.v * rhs.v


def main():
    var x = P(3)
    x *= x
    print(x.v)
