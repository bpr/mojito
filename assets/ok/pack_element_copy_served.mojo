# A pack element's `.copy()` under a served pack loop calls the element's
# copy, in a pack-keyed `def` and in a method keyed on a pack of its own,
# so the copy and the argument are each destroyed once.
def go[*Ts: Writable & Copyable & Deinitable](*a: *Ts):
    comptime for i in range(Ts.length):
        var x = a[i].copy()
        print(x)


struct Fwd:
    def __init__(out self):
        pass

    def go[*Ts: Writable & Copyable & Deinitable](self, *a: *Ts):
        comptime for i in range(Ts.length):
            var x: Ts[i] = a[i].copy()
            print(x)


def main():
    go(1, "y")
    Fwd().go(2, "z")
