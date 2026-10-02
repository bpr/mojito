# An owned `var *xs` collector takes a transferred place, a temporary, and an
# explicit copy of a type that is not `ImplicitlyCopyable`; a place of an
# `ImplicitlyCopyable` type is copied in.
def count(var *xs: List[Int]) -> Int:
    return len(xs) * 10 + len(xs[0])


def width(var *parts: String) -> Int:
    return len(parts) * 10 + parts[0].byte_length()


def main():
    var a: List[Int] = [1, 2]
    var b: List[Int] = [3]
    print(count(a.copy(), b^, [4, 5, 6]))
    print(len(a))
    var s = String("x")
    print(width(s, "yz"))
    print(s)
    var t = (a^, 1)
    print(len(t[0]), t[1])
