# An aggregate storing an origin-bearing pointer carries the owner loan; field
# derefs go through the stored frame/slot handle and alias the owner.
@fieldwise_init
struct Borrowed[origin: Origin]:
    var ptr: Pointer[Int, Self.origin]

def main():
    var value = 42
    var b = Borrowed(Pointer(to=value))
    print(b.ptr[0])
    b.ptr[0] = 9
    print(value)
