# expect: propagating the write requirement
# A receiver that leaves the written origin parameter symbolic (a wrapper
# generic over the same origin) cannot discharge the write requirement; the
# transitive propagation is a recorded subset limitation.
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

def main():
    var data = List[Int]()
    data.append(7)
    var w = Wrap(View(data))
    w.poke()
