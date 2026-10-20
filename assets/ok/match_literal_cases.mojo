# `match`/`case` with literal, `== expr`, wildcard, and bare-binding
# patterns: the subject is evaluated once, the first matching case runs and
# the rest are skipped, a statement with no matching case falls through, and
# `case Int` is an identifier binding, not a type test.
def subject() -> Int:
    print("subject")
    return 7


def describe(n: Int) -> String:
    match n:
        case 0:
            return "zero"
        case 1:
            return "one"
        case _:
            return "many"


def main():
    match subject():
        case 1:
            print("one")
        case 7:
            print("seven")
        case _:
            print("other")
    print(describe(0), describe(1), describe(5))
    match 2:
        case 1:
            print("one")
    print("fallthrough")
    var x = 10
    match 5:
        case Int:
            print("bound", Int, x)
    match "x":
        case "y":
            print("y")
        case "x":
            print("x")
    var f = 2.5
    match f:
        case 2.5:
            print("float")
        case _:
            print("nf")
    var expected = 3
    match 3:
        case == (expected - 1):
            print("two")
        case == expected:
            print("three")
    match True:
        case False:
            print("f")
        case True:
            print("t")
