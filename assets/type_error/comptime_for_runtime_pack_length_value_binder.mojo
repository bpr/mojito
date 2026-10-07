# `len(args)` of a pack collector is a runtime value beside a `Float64`
# binder too, as it is in the pinned Mojo, so it cannot bound a
# `comptime for`.
def show[tag: Float64, *Ts: Writable](*args: *Ts):
    comptime for i in range(len(args)):
        print(args[i], tag)


def main():
    show[1.5](1, "a")
