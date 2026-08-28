# expect: writes through an origin parameter bound to an immutable source
# A parametric-mut write is rejected at the call site whose receiver binds the
# origin parameter to an immutable source (a read parameter's storage).
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def bump(mut self):
        self.src[0] += 1

def poke(data: List[Int]):
    var v = View(data)
    v.bump()

def main():
    var source = List[Int]()
    source.append(7)
    poke(source)
