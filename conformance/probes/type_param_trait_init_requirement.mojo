# Pin gap probe (Mojo 1.2.0.dev2026092105): constructing a type parameter
# through a user trait's `__init__` requirement with an argument. The pin
# prints `8`; Mojito reports it unsupported. Roadmap entry R283.
trait Make:
    def __init__(out self, v: Int):
        ...

    def val(self) -> Int:
        ...


struct A(Make, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def val(self) -> Int:
        return self.v * 2


def build[T: Make & Movable & Deinitable](v: Int) -> T:
    return T(v)


def main():
    print(build[A](4).val())
