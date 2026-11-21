@fieldwise_init
struct Borrowed[origin: Origin]:
    var ptr: Pointer[Int, Self.origin]

struct ExternallyManaged:
    var ptr: Pointer[Int, MutUntrackedOrigin]

def main():
    var value = 42
    var direct = Pointer(to=value)
    print(direct[0])
    direct[0] = 7
    var b = Borrowed(Pointer(to=value))
    print(b.ptr[0])
    b.ptr[0] = 9
    print(value)
