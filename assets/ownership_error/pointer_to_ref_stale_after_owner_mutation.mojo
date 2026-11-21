# expect: conflicts with live reference
# A mutation of the owner while a pointer taken through a `ref` binding to
# one of its fields is live. The pin runs it and prints 4; Mojito's pointer
# loans the referent, so the mutating call conflicts with it (R1160).
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
