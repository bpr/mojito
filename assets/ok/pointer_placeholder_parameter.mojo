# A bare placeholder-origin pointer parameter (`Pointer[T, _]`) binds any
# provenance and holds no loan: upstream infers the origin (and so the
# pointer's `mut`) per call, so the body reads through it but cannot prove
# the capability a write needs — Mojito reads the bare spelling as the
# immutable alias. A bare parameter forwards to another bare one.
def first(bytes: Pointer[UInt8, _], n: Int) -> Int:
    return n

def peek(p: Pointer[Int, _]) -> Int:
    return p[unsafe_offset=0]

def via(p: Pointer[Int, _]) -> Int:
    return peek(p)

def read(x: Int) -> Int:
    return peek(Pointer(to=x))

def main():
    var s = String("hello")
    print(first(s.unsafe_ptr(), 5))
    var x = 4
    print(peek(Pointer(to=x)), via(Pointer(to=x)), read(x))
