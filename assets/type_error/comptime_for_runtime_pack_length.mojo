# `len(args)` of a pack collector is a runtime value, as it is in the pinned
# Mojo, so it cannot bound a `comptime for`; the compile-time spellings are
# `args.__len__()`, `Ts.length`, and `len(Ts)`.
def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(len(args)):
        print(args[i])


def main():
    show(1, "two")
