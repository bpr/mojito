# `continue`/`break`/`return` inside a case body target the enclosing loop
# or function; an exhaustive `Bool` match and a `_` case satisfy return
# analysis.
def f(xs: List[Int]) -> Int:
    for x in xs:
        match x:
            case 0:
                continue
            case 9:
                break
            case 5:
                return 50
            case _:
                print("x", x)
    return -1


def g(x: Int) -> Int:
    match x:
        case 1:
            return 10
        case _:
            return 20


def k(x: Bool) -> Int:
    match x:
        case True:
            return 10
        case False:
            return 20


def main():
    print(f([0, 1, 9, 2]))
    print(f([3, 5]))
    print(g(1), g(2), k(True))
    var n = 0
    while True:
        match n:
            case 3:
                break
            case _:
                n += 1
    print(n)
