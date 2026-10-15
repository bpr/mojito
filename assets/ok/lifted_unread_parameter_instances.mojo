# A function lifted from a compile-time expression is a generator over the
# binders it declares, as on the pin: an evaluated default or a `comptime
# if` condition that reads only some of its owner's parameters has one
# instance, and one compile-time evaluation, per assignment of those it
# reads, however the others vary.
def is_big(k: Int) -> Bool:
    return k > 10


def scaled[n: Int, m: Int](x: Int = m * 2) -> Int:
    return x + n


def pick[n: Int, m: Int]() -> Int:
    comptime if is_big(m):
        return n
    return n + 1


def typed[T: AnyType, m: Int](x: Int = m + 1) -> Int:
    return x


struct Box[n: Int, m: Int]:
    @staticmethod
    def get(x: Int = Self.m * 3) -> Int:
        return x + Self.n


def main():
    print(scaled[1, 3]())
    print(scaled[2, 3]())
    print(scaled[1, 4]())
    print(pick[1, 20]())
    print(pick[2, 20]())
    print(pick[3, 5]())
    print(typed[Int, 2]())
    print(typed[Bool, 2]())
    print(Box[1, 2].get())
    print(Box[5, 2].get())
