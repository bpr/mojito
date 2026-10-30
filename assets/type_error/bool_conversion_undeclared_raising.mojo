# expect: does not conform to trait 'Boolable'
# `Bool(x)` binds `Bool.__init__[T: Boolable]`, so a struct whose
# `__bool__` raises, and which therefore cannot declare `Boolable`, does not
# convert, as at the pin.
struct B:
    def __init__(out self):
        pass

    def __bool__(self) raises -> Bool:
        return True


def main() raises:
    print(Bool(B()))
