# A trait method's docstring may precede its requirement's `...`; before
# statements it begins a default body.


trait Tr:
    """A documented trait."""

    def f(self) -> Int:
        """Return the value."""
        ...

    def g(self) -> Int:
        """Return twice the value, by default."""
        return self.f() * 2


@fieldwise_init
struct S(Tr):
    var x: Int

    def f(self) -> Int:
        return self.x


def main():
    var s = S(3)
    print(s.f(), s.g())
