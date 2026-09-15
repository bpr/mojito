# expect: aliasing values passed mutably to 'self' argument and passed mutably to 'sink' argument in 'stash' call
# The receiver participates in argument exclusivity: a `mut self` whose own
# origin binder is bound to `view` and a `mut sink` carrying boxes over
# `view` may not reach one call, as at the pin.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Stasher[o: Origin[mut=True]]:
    var count: Int

    def stash(mut self, mut sink: List[RefBox[Self.o]], var box: RefBox[Self.o]):
        self.count += 1
        sink.append(box^)

def main():
    var local: List[Int] = [9]
    ref view = local
    var s = Stasher[origin_of(view)](0)
    var sink = List[RefBox[origin_of(view)]]()
    s.stash(sink, RefBox(Pointer(to=view)))
    print(len(sink))
