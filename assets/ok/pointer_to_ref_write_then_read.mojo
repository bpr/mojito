# A pointer constructed through a `ref` binding addresses the binding's
# referent with its mutability: a write through it and a read after the
# write both run, printing 9, as at the pin.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = Pointer(to=r)
    p[] = 9
    print(p[])
