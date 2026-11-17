# expect: returned reference escapes storage outside its declared origin
# A function returning through its collector's pack origin cannot return a
# reference to another parameter, as the pinned Mojo rejects it ("cannot
# return reference with incompatible origin").
def other[*Ts: Writable](ref a: Int, *args: *Ts) -> ref[args.origin] Int:
    return a


def main():
    var x = 1
    var s = String("a")
    print(other(x, s))
