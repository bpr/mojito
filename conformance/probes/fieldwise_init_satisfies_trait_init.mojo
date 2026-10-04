# Pin gap probe (Mojo 1.2.0.dev2026092105): a `@fieldwise_init` initializer
# satisfies a trait's `__init__` requirement. The pin prints `8`; Mojito
# reports "declares conformance to trait 'Make' but is missing method
# '__init__'". Roadmap entry R284.
trait Make:
    def __init__(out self, v: Int):
        ...

    def val(self) -> Int:
        ...


@fieldwise_init
struct A(Make, Movable):
    var v: Int

    def val(self) -> Int:
        return self.v * 2


def build[T: Make & Movable & Deinitable](v: Int) -> T:
    return T(v)


def main():
    print(build[A](4).val())
