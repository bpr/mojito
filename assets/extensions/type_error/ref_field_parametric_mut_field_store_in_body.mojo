# expect: reference returned by '__getitem__' whose origin mutability is not known here
# A field store into a parametric-mut ref field's element (`Origin[mut=m]`)
# rejects in the method body, as upstream judges the binder there: `m` is
# not provably `True`, however a caller binds it.
@fieldwise_init
struct C(Copyable, Movable):
    var n: Int

@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[C]

    def put(mut self):
        self.src[0].n = 3

def poke(mut data: List[C]):
    var v = View(data)
    v.put()

def main():
    var source = List[C]()
    source.append(C(7))
    poke(source)
