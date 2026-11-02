# expect: invalid call to 'Bool': no constructor overload matches
# `Bool(x)` is upstream's `Bool.__init__[T: Boolable]`, so a struct with a
# `__bool__` that does not declare `Boolable` is rejected.
@fieldwise_init
struct Truthy(Copyable, Movable):
    var x: Int

    def __bool__(self) -> Bool:
        return self.x != 0


def main():
    print(Bool(Truthy(1)))
