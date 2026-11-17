# A `read` collector's element reference (`args[0]`, `ref[Self.origin]` over
# the collector's `VariadicPack`) names the function's implicit pack origin,
# `args.origin`, never one of the origins the function declares: returning it
# through `ref[o1]` rejects even though `o1` is the function's second binder.
# expect: escapes storage outside its declared origin
def pick[o0: ImmOrigin, o1: ImmOrigin, *Ts: Writable](
    p: Pointer[Int, o1], *args: *Ts
) -> ref[o1] Ts[0]:
    return args[0]


def main():
    var x = 1
    print(pick(Pointer(to=x), 7, "a"))
