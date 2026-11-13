# A struct's type-valued parameterized member applied through `Self` in a
# later member binds its bracket list against the member's own parameters:
# one too many arguments is the pin's error.
# expect: type 'W[T].Get' expects 1 type argument(s), got 2


struct W[T: AnyType]:
    comptime Get[i: Int] = Self.T
    comptime First = Self.Get[0, 1]


def main():
    var x: W[Int].First = 3
    print(x)
