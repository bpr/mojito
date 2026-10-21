# `comptime match` coverage reads the same command shapes a runtime `match`
# does: a duplicate literal and a case after a catch-all warn, and the
# program runs; a non-exhaustive `Bool` match warns; a type subject's
# `== Int` keeps no spelling, so a repeated one does not warn; a concluding
# `Bool` match whose cases return satisfies return analysis, and an untaken
# arm that initializes a variable joins the rest.
def k[b: Bool]() -> Int:
    comptime match b:
        case True:
            return 10
        case False:
            return 20


def init[n: Int]() -> Int:
    var r: Int
    comptime match n:
        case 1:
            r = 10
        case _:
            r = 20
    return r


def same[T: AnyType]() -> String:
    comptime match T:
        case == Int:
            return "int"
        case == Int:
            return "again"
        case _:
            return "other"


def main():
    comptime n = 3
    comptime match n:
        case 1:
            print("a")
        case 1:
            print("b")
        case _:
            print("c")
        case 4:
            print("d")
    comptime b = True
    comptime match b:
        case True:
            print("t")
    print(k[True](), k[False](), init[1](), init[2](), same[Int](), same[Bool]())
