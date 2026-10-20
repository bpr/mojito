# PROBE: a trait method carries a caller-contract decorator.
#
# The pin runs it and prints:
#   9
#
# Mojito rejects it: "decorator '@__unsafe_nested_origins_read_only' on a
# trait method" is unsupported (R1165).
trait Shape:
    @__unsafe_nested_origins_read_only
    def area(self) -> Int:
        ...

@fieldwise_init
struct Sq(Shape):
    var s: Int

    def area(self) -> Int:
        return self.s * self.s

def main():
    print(Sq(3).area())
