# StringLiteral and the nominal String are distinct overload keys (as
# upstream): a literal argument selects the exact `StringLiteral` overload,
# a String argument the `String` one, and a `StringLiteral` parameter never
# accepts a String.
def f(x: StringLiteral) -> Int:
    return 1

def f(x: String) -> Int:
    return 2

def main():
    print(f("a"))
    print(f(String("b")))
    var s = String("c")
    print(f(s))
