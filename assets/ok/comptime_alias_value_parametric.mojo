# A value-bodied generic `comptime` alias, annotated or not, applied to a
# literal, to a parameter still open, by keyword, inside another alias's
# body, as a struct's parameter argument, and in a local constant; a
# Bool-bodied one reads as a value too.
comptime plus_one[n: Int]: Int = n + 1
comptime twice[n: Int] = n * 2
comptime third[n: Int]: Int = twice[n] + 1
comptime positive[n: Int] = n > 0


@fieldwise_init
struct Arr[n: Int]:
    def size(self) -> Int:
        return Self.n


def bumped[k: Int]() -> Int:
    return plus_one[k]


def is_positive[k: Int]() -> Bool:
    return positive[k]


def doubled_size[k: Int]() -> Int:
    var a = Arr[twice[k]]()
    return a.size()


def main():
    print(plus_one[3])
    print(bumped[4]())
    print(plus_one[n=9])
    print(third[3])
    print(doubled_size[5]())
    comptime z = twice[4]
    print(z)
    comptime w = twice[n=6]
    print(w)
    print(positive[3])
    print(is_positive[-1]())
