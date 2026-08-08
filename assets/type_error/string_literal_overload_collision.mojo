# expect: already declared
# StringLiteral and the nominal String share the stable `String` overload
# symbol spelling, so a pair differing only in that type is a redeclaration.
def f(x: StringLiteral) -> Int:
    return 1

def f(x: String) -> Int:
    return 2

def main():
    print(f("a"))
