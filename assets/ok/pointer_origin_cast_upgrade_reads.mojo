# Casting an immutable Pointer to a mutable origin is accepted, but the result
# keeps the immutable capability: reads through it work, writes do not.
from std.memory.alloc import unsafe_alloc

def peek(p: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    var q = p.unsafe_origin_cast[MutUntrackedOrigin]()
    return q[]

def main():
    var p = unsafe_alloc[Int](1)
    p[] = 41
    print(peek(p))
    p.free()
