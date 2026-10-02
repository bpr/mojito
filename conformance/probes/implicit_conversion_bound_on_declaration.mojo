# PROBE (divergence): an `@implicit` conversion inside a generic method is
# bound on the declaration.
#
# The pin selects the constructor while it checks `wrap`, where the source
# is `Self.T`, so every instance runs the `Self.T` constructor and this
# prints 2 twice. Mojito selects again per instance, finds both constructors
# applicable for `Box[Int]`, and reports "ambiguous implicit conversion from
# 'Int' to 'Wrapper[Int]'". Filed in `docs/roadmap.md` §3 ("An implicit
# conversion in a generic method is selected again per instance"). When
# Mojito prints 2 twice, promote this file to `assets/ok/`.
#
# Run:    mojo run implicit_conversion_bound_on_declaration.mojo
#         cargo run -- run conformance/probes/implicit_conversion_bound_on_declaration.mojo
struct Wrapper[T: ImplicitlyCopyable & Deinitable]:
    var tag: Int

    @implicit
    def __init__(out self, value: Int):
        self.tag = 1

    @implicit
    def __init__(out self, value: Self.T):
        self.tag = 2


@fieldwise_init
struct Box[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T

    def wrap(self) -> Int:
        var w: Wrapper[Self.T] = self.item
        return w.tag


def main():
    var b = Box[Int](3)
    print(b.wrap())
    var s = Box[String]("x")
    print(s.wrap())
