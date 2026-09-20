# One expression through a value parameter's default and an ordinary
# `comptime` binding folds alike: both evaluate through the shared folder, so
# `//` and `%` floor toward the divisor's sign on every route.
struct Holder[n: Int, m: Int = n // -2](Copyable, Movable):
    var v: Int

    def __init__(out self):
        self.v = Self.m


def main():
    comptime direct = Int(7) // Int(-2)
    comptime remainder = Int(7) % Int(-2)
    var holder = Holder[7]()
    print(direct, holder.v)
    print(remainder, Int(-7) % Int(2))
