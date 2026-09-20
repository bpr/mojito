# A trait-bound generic whose body names none of its parameters is checked
# once, with `T` symbolic. Each instance inherits the checked template's facts
# instead of being checked again as a clone (`docs/notes/
# instantiation-from-template.md`, class ClosedScalarBody).
def tag[T: Copyable](x: T) -> Int:
    return 7


def main():
    print(tag(3))
    print(tag(True))
