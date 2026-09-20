# expect: ambiguous overloaded method call
# Two methods with a homogeneous `*rest: String` collector differ by one
# regular `String` parameter. It binds by reference either way, so the call is
# ambiguous, as the pinned Mojo reports.
struct Holder:
    def __init__(out self):
        pass

    def g(self, a: Int, *rest: String) -> Int:
        return 2

    def g(self, a: Int, b: String, *rest: String) -> Int:
        return 3


def main():
    var x = 1
    var s = String("s")
    var holder = Holder()
    print(holder.g(x, s, s))
