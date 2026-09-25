# Pin gap probe (Mojo 1.2.0.dev2026092105): an imported `comptime` alias of a
# struct application (`default_hasher`) as a parameter type of a generic
# struct's method. The pin prints True; Mojito reports "unknown type
# '__module$hasher$default_hasher'" for the instance, though the same
# annotation resolves on a module-level `def` and on a plain struct's method.
# Roadmap section 3 carries the entry.
from std.hashlib import default_hasher


struct Box[T: Copyable & Deinitable & Hashable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def feed(self, mut hasher: default_hasher):
        self.item.__hash__(hasher)


def main():
    var hasher = default_hasher()
    Box[Int](3).feed(hasher)
    print(hasher^.finish() > 0)
