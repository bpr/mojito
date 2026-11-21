# expect: operator '-' is not defined for Pointer[Int, MutUntrackedOrigin] and IntLiteral[1]
# Upstream's `Pointer.__sub__` takes another pointer and returns the element
# distance, so subtracting an integer from a pointer is rejected, as at the
# pin; `p.unsafe_offset(-1)` steps backwards.
from std.memory.alloc import unsafe_alloc


def main():
    var p = unsafe_alloc[Int](2)
    var q = p.unsafe_offset(1)
    print(Int(q - 1) > 0)
    p.unsafe_free()
