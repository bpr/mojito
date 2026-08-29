# expect: writes through an origin parameter bound to an immutable source
# The wrapper preserves the nested view's write requirement, so an immutable
# concrete source rejects at the outer call site.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def bump(mut self):
        self.src[0] += 1

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var view: View[o]

    def poke(mut self):
        self.view.bump()

def poke(data: List[Int]):
    var w = Wrap(View(data))
    w.poke()

def main():
    var data = List[Int]()
    data.append(7)
    poke(data)
