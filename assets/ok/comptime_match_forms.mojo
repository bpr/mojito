# `comptime match` over a type parameter with `== Int`, over a value with
# literals, over a tuple with a binding, and with `1 | 3 as k if k > 2`: each
# case's tests and guard are one compile-time condition of a `comptime if`
# chain, its bindings compile-time values, and the elaborator keeps the
# first case whose condition holds.
def kind[T: AnyType]() -> String:
    comptime match T:
        case == Int:
            return "integer"
        case == String:
            return "string"
        case _:
            return "other"


def main():
    print(kind[Int](), kind[String](), kind[Bool]())
    comptime n = 3
    comptime match n:
        case 1:
            print("one")
        case 3:
            print("three")
        case _:
            print("many")
    comptime match (n, n + 1):
        case (3, m):
            print("m", m)
        case _:
            print("other")
    comptime match n:
        case 1 | 3 as k if k > 2:
            print("guarded", k)
        case _:
            print("no")
