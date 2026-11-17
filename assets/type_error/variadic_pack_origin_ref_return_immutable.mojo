# expect: expression must be mutable in assignment
# A `read` collector's pack origin is `Origin[mut=False]`, so a reference
# returned through `ref[args.origin]` cannot be written, as the pinned Mojo
# rejects it.
def first[*Ts: Writable](*args: *Ts) -> ref[args.origin] Ts[0]:
    return args[0]


def main():
    var x = 1
    var s = String("a")
    first(x, s) = 5
    print(x)
