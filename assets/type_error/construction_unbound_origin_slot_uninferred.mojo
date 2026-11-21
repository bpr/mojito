# A construction whose initializer's arguments bind none of the struct's
# origin parameters, with no application supplying them, fails to infer
# them: the result type does not decide a parent struct's parameters.
# expect: failed to infer parameter 'mut' of parent struct 'V'
struct V[mut: Bool, //, origin: Origin[mut=mut]]:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k


def main():
    var l = 3
    var v: V[origin_of(l)] = V(5)
    print(v.k)
