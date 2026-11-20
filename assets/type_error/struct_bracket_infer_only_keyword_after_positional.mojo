# expect: unexpected keyword parameter 'o'
# A struct application binds its whole bracket as a call's does: an
# infer-only origin binder keyed after a positional argument is unexpected.
struct H[o: ImmOrigin, //, T: AnyType]:
    var n: Int

    def __init__(out self):
        self.n = 1


def main():
    var x = 5
    var h = H[Int, o=origin_of(x)]()
    print(h.n)
