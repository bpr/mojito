# expect: cannot implicitly convert 'IntLiteral[3]' value to 'Movable'
# A module `comptime` annotated with a trait declares a type, and a value
# body converts to no trait metatype.
comptime X: Movable = 3


def main():
    print("x")
