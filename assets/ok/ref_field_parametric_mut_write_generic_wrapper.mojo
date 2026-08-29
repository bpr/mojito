# A write requirement inherited from a wrapped parametric-origin view propagates
# through the wrapper method and is discharged at its concrete call site.
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
    print(data[0])
# stdout: 8
