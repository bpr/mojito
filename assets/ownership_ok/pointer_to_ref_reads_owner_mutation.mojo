# A mutation of the owner while a pointer taken through a `ref` binding to
# one of its fields is live. The pointer names the field, which the mutation
# writes in place, so it runs and the pointer reads 4, as at the pin (R1160).
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

    def bump(mut self):
        self.a += 1

def main():
    var t = Pair(3, 4)
    ref r = t.a
    var p = Pointer(to=r)
    t.bump()
    print(p[])
