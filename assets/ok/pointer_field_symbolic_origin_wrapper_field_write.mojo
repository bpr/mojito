# The binder resolves through a field chain: `w.view` carries the origins
# `Wrap(View(data))` bound.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var view: View[Self.o]

def main():
    var data = List[Int]()
    data.append(7)
    var w = Wrap(View(data))
    w.view.src[][0] = 8
    print(data[0])
