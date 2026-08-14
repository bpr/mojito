# `Pointer(to=r)` over a `ref` binding mints a pointer whose provenance is
# the conservative subtree of the reference's origin — the referent is that
# base or some descendant. Reads work through a local `ref` and through a
# `ref` parameter's symbolic origin alike.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

def peek(ref x: Int) -> Int:
    var p = UnsafePointer(to=x)
    return p[]

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = UnsafePointer(to=r)
    print(p[])
    print(peek(t.b))
