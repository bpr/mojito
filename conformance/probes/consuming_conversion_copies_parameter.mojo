# PROBE (divergence): an `@implicit` conversion through a constructor that
# consumes its source, applied to a place of a parameter type that is only
# `Copyable`.
#
# The pin rejects the declaration: "value of type 'T' cannot be implicitly
# copied, it does not conform to 'ImplicitlyCopyable'". Mojito's check of
# `boxed` records no copy at the conversion, so the program runs and prints
# 7 twice, copying a `List[Int]` implicitly. Filed in `docs/roadmap.md` §3
# ("An implicit conversion through a consuming constructor copies a
# parameter-typed place"). When Mojito rejects it, promote this file to
# `assets/type_error/` with the pin's diagnostic.
#
# Run:    mojo run consuming_conversion_copies_parameter.mojo
#         cargo run -- run conformance/probes/consuming_conversion_copies_parameter.mojo
struct Wrapper[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var value: Self.T

    @implicit
    def __init__(out self, var value: Self.T):
        self.value = value^

struct Holder[T: Copyable & Deinitable](Deinitable, Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def keep(self, box: Wrapper[Self.T]) -> Int:
        return 7

    def boxed(self) -> Int:
        return self.keep(self.item)

def main():
    var number = Holder[Int](5)
    print(number.boxed())
    var lists = Holder[List[Int]](List[Int]())
    print(lists.boxed())
