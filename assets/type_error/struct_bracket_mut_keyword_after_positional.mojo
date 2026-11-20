# expect: unexpected keyword parameter 'mut'
# An origin's `mut=` binder is infer-only, so it binds by keyword only ahead
# of the positional arguments.
struct H[mut: Bool, //, T: AnyType, o: Origin[mut=mut]]:
    var n: Int

    def __init__(out self):
        self.n = 1


def main():
    var x = 5
    var h = H[Int, origin_of(x), mut=True]()
    print(h.n)
