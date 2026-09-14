# expect: expression must be mutable for in-place operator destination
# The wrapped view's `bump` writes in its own generic body, so it rejects
# there before any call site could discharge it.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def bump(mut self):
        self.src[][0] += 1

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var view: View[Self.o]

    def poke(mut self):
        self.view.bump()

def main():
    var data = List[Int]()
    data.append(7)
    var w = Wrap(View(data))
    w.poke()
    print(data[0])
