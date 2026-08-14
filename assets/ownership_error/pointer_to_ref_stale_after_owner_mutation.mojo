# expect: invalidated interior reference
# The subtree provenance minted by `Pointer(to=ref_binding)` stales on any
# mutation of the owner at or below the reference's base.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

    def bump(mut self):
        self.a += 1

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = UnsafePointer(to=r)
    t.bump()
    print(p[])
