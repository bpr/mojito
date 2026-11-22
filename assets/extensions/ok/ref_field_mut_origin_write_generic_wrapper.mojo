# A wrapper over a `MutOrigin` view calls the view's writing method from its
# own body: both binders are mutable there, and the write lands in the
# borrowed storage.
@fieldwise_init
struct View[o: MutOrigin]:
    var src: ref[o] List[Int]

    def bump(mut self):
        self.src[0] += 1

@fieldwise_init
struct Wrap[o: MutOrigin]:
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
