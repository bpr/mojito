# Pin gap probe (Mojo 1.3.0.dev2026111316): a lambda with an empty capture
# list (`lambda (x: Int) {} -> Int`) at a `def(...)` bound. The pin types it
# as a capture-free function and prints 0, 15, 0, 15; Mojito types it as a
# thin function value riding a register and prints 16 for its `size_of`.
# docs/roadmap.md R2314.
from std.sys import size_of


def transform[F: def(x: Int) -> Int](f: F, v: Int) -> Int:
    print(size_of[F]())
    return f(v)


def main():
    print(transform(lambda (x: Int) {} -> Int: x * 3, 5))
    print(transform(lambda (x: Int) -> Int: x * 3, 5))
