# A `with` statement inside a `match` case body is desugared like any other.
struct M:
    def __init__(out self):
        pass

    def __enter__(self) -> Int:
        print("enter")
        return 1

    def __exit__(self):
        print("exit")


def main():
    var x = 1
    match x:
        case 1:
            with M() as v:
                print("body", v)
        case _:
            pass
