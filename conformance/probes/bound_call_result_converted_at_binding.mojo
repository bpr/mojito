# Does a generic body bind a bound method's call result through an
# `@implicit` conversion at an annotated `var`?
#
# The pin prints `2 a`. Mojito rejects the generic body before it runs
# ("register r1 has no checked type" for the `copy` call); the same binding
# at a concrete type, and the result bound to an unannotated local first,
# both run.
def f[T: Copyable & Deinitable](v: T) -> Int:
    var x: Optional[T] = v.copy()
    _ = x^
    return 2


def main():
    var s = String("a")
    var x: Optional[String] = s.copy()
    print(f(3), x.value())
