# A store through a dereferenced pointer whose pointee type carries a loan
# keeps the borrowed owner alive for as long as the pointer is used: each
# span stored into the allocation reads the list it borrowed, and a second
# allocation over another list keeps its own owner apart.
from std.memory.alloc import unsafe_alloc

def main():
    var xs: List[Int] = [4, 5, 6]
    var ys: List[Int] = [7, 8]
    var q = unsafe_alloc[Span[Int, origin_of(xs)]](2)
    q[] = Span(xs)
    q[unsafe_offset=1] = Span(xs)[1:]
    print(q[][1], q[unsafe_offset=1][0], len(q[unsafe_offset=1]))
    var r = unsafe_alloc[Span[Int, origin_of(ys)]](1)
    r[] = Span(ys)
    print(r[][1])
    q.unsafe_free()
    r.unsafe_free()
    print(len(xs), len(ys))
