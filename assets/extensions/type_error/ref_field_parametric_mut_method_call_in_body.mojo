# expect: reference-returning method receiver
# A `mut self` method call on a parametric-mut ref field's element
# (`Origin[mut=m]`) rejects in the method body, as upstream judges the
# binder there: `m` is not provably `True`, however a caller binds it.
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

def poke(mut data: List[C]):
    var v = View(data)
    v.bump()

def main():
    var source = List[C]()
    source.append(C(7))
    poke(source)
