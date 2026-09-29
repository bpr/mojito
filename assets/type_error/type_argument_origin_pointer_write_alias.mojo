# expect: aliasing values passed mutably to 'self' argument and passed mutably to 'value' argument in 'unsafe_write' call
# The loan a pointee type's origin carries counts in argument exclusivity:
# a `Pointer[Span[Int, origin_of(xs)], MutUntrackedOrigin]` and the span
# written through it both reach the mutable `xs`, as at the pin.
from std.memory.alloc import unsafe_alloc


def main():
    var xs: List[Int] = [4, 5, 6]
    var p = unsafe_alloc[Span[Int, origin_of(xs)]](1)
    p.unsafe_write(Span(xs))
    print(p[0][1])
    p.free()
