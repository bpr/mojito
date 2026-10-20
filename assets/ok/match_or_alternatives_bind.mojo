# `|` alternatives bind the same `var` name from different projections, and
# `as` binds the whole `|` (`3 | 4 as n` is `(3 | 4) as n`).
def classify(p: Tuple[Int, Int]) -> Int:
    match p:
        case (0, var x) | (var x, 1):
            return x * 10
        case (2, 2) | (3, 3):
            return -1
        case _:
            return 0


def main():
    print(classify((0, 4)))
    print(classify((5, 1)))
    print(classify((0, 1)))
    print(classify((3, 3)))
    print(classify((9, 9)))
    var s = String("b")
    match s:
        case "a" | "b" as which:
            print("ab", which)
        case _:
            print("other")
    match 3:
        case (1 | 2) as n:
            print("low", n)
        case 3 | 4 as n:
            print("high", n)
