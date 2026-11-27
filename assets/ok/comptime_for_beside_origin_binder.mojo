# A compile-time expression in a body whose declaration has an origin
# binder lifts a function over the argument binders alone, as upstream's
# `print` iterates its pack beside its `sep` and `end` views.
def show[o: ImmOrigin, //, *Ts: Writable](*values: *Ts, sep: StringSpan[o] = " "):
    comptime for i in range(values.__len__()):
        comptime if i > 0:
            print(sep, end="")
        print(values[i], end="")
    print()


def main():
    show(1, 2, 3)
    show(1, 2, 3, sep=",")
