# A pack element constructed at an index a local `comptime` binding aliases
# (`comptime k = i`, then `Ts[k]()`), and a `comptime for` over an
# explicit `range(...).__iter__()` whose bound is a collector's
# `args.__len__()`: both read the loop's element through upstream's
# `paramfor_next_value`, as the pin does.
def build[*Ts: Movable & Defaultable & Writable & Deinitable]():
    comptime for i in range(len(Ts)):
        comptime k = i
        var value = Ts[k]()
        print(k, value)


def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()).__iter__():
        print(i, args[i])


def main():
    build[Int, Bool, String]()
    show(1, "two", 3.5)
