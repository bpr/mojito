# Current Mojo's pointer-backed Span construction: `Span(unsafe_ptr=p,
# length=n)` takes `Pointer[Self.T, Self.origin]`, so a tracked pointer
# (`xs.unsafe_ptr()`, a `Pointer(to=x)`) binds the span's origin and the span
# loans that source; an explicit application (`Span[Int, origin_of(self)]`)
# is checked against the pointer's provenance; an untracked heap pointer
# binds the slot untracked (the caller vouches for the storage).
from std.memory import unsafe_alloc

struct Buf:
    var items: List[Int]

    def __init__(out self):
        self.items = [10, 20, 30]

    def view(ref self) -> Span[Int, origin_of(self)]:
        return Span[Int, origin_of(self)](unsafe_ptr=self.items.unsafe_ptr(), length=len(self.items))

def total(s: Span[Int, _]) -> Int:
    var acc = 0
    for x in s:
        acc += x
    return acc

def main():
    var buf = Buf()
    var v = buf.view()
    print(len(v), v[1], total(v))

    var xs: List[Int] = [4, 5, 6]
    var s = Span(unsafe_ptr=xs.unsafe_ptr(), length=len(xs))
    print(s[2], total(s))

    var x = 9
    var one = Span(unsafe_ptr=Pointer(to=x), length=1)
    print(one[0])

    var p = unsafe_alloc[Int](2)
    p[0] = 7
    p[1] = 8
    var raw: Span[Int, MutUntrackedOrigin] = Span(unsafe_ptr=p, length=2)
    print(raw[0], raw[1], total(raw))
    p.free()

    var text = String("hello")
    var bytes = text.as_bytes()
    print(len(bytes), Int(bytes[0]))
