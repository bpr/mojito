# expect: writes through an origin parameter bound to an immutable source
# A `mut self` method call on a parametric-mut ref field's element is a write
# through the view, rejected at the call site whose receiver binds the origin
# parameter to an immutable source (a read parameter's storage).
@fieldwise_init
struct C(Copyable, Movable):
    var n: Int

    def inc(mut self):
        self.n += 1

@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[C]

    def bump(mut self):
        self.src[0].inc()

def poke(data: List[C]):
    var v = View(data)
    v.bump()

def main():
    var source = List[C]()
    source.append(C(7))
    poke(source)
