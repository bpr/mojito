# expect: cannot write through a Pointer with an immutable origin
# A bare `ref` parameter propagates caller mutability but cannot assume
# write permission, so the subtree pointer minted from it is not statically
# mutable and writes reject.
def poke(ref x: Int):
    var p = UnsafePointer(to=x)
    p[] = 9

def main():
    var v = 42
    poke(v)
    print(v)
