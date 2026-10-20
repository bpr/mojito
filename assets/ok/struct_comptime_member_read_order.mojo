# A struct's `comptime` members read one another in any order, as at the
# pin: a member reads a sibling declared after it, a later member's
# application, and a default its trait supplies.
trait HasK:
    comptime K: Int = 3


struct D(HasK):
    comptime X = Self.K + 1


struct E:
    comptime X = Self.Y * 2
    comptime Twice = Self.Of[2] + Self.K
    comptime Y = Self.K + 1
    comptime Of[n: Int] = n * Self.K
    comptime K = 5


def main():
    print(D.X)
    print(E.X, E.Y, E.Twice)
