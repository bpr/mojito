# Pin gap probe (Mojo 1.2.0.dev2026092105): a call whose only argument
# reaches its parameter through an `@implicit` constructor, where the
# parameter's type is built over the callee's own binder. The pin infers
# `U = T` from the conversion's target and prints 3; Mojito reports "cannot
# infer type parameter 'U' of 'boxed' from the arguments", because inference
# matches the argument's own type against the parameter's before any
# conversion is considered. Roadmap section 3 carries the entry.
struct Wrapper[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var value: Self.T

    @implicit
    def __init__(out self, value: Self.T):
        self.value = value.copy()


def boxed[U: Copyable & Deinitable](box: Wrapper[U]) -> Int:
    return 3


def wrap[T: Copyable & Deinitable](value: T) -> Int:
    return boxed(value)


def main():
    print(wrap[Int](1))
