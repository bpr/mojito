# expect: invalidated interior reference
# A pointer minted through a `ref` binding is a mutable subtree reference:
# its first write succeeds and any later use rejects.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = UnsafePointer(to=r)
    p[] = 9
    print(p[])
