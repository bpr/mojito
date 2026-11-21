# expect: augmented assignment '+' requires an in-place method on 'Pointer[Int, MutUntrackedOrigin]'
# Upstream's `Pointer` declares `__add__` but no `__iadd__`, and augmented
# assignment does not fall back to the binary operator, so `p += 1` over a
# pointer is rejected, as at the pin.
from std.memory.alloc import unsafe_alloc


def main():
    var p = unsafe_alloc[Int](2)
    p += 1
    p.unsafe_free()
