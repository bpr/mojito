# A single write through a mutable subtree pointer minted from a `ref`
# binding reaches the owner's storage; the pointer is not used again, so
# the first-write self-invalidation never observes a later use.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = UnsafePointer(to=r)
    p[] = 9
    print(t.a)
