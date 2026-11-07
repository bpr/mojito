# expect: writes through an origin parameter bound to an immutable source
# A field store into a parametric-mut ref field's element is a write through
# the view, rejected at the call site whose receiver binds the origin
# parameter to an immutable source (a read parameter's storage).
@fieldwise_init
struct C(Copyable, Movable):
    var n: Int

@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[C]

    def put(mut self):
        self.src[0].n = 3

def poke(data: List[C]):
    var v = View(data)
    v.put()

def main():
    var source = List[C]()
    source.append(C(7))
    poke(source)
