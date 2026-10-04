# A `def` keyed on a type pack is served by its template: the checker types
# the body once with `values[i]` at the dependent element `Ts[i]` and the
# pack's length as the loop's bound, and the elaborator binds the pack from
# each call and unrolls the loop per instance. Three elements, one, and none.
def show[*Ts: Writable](*values: *Ts):
    comptime for i in range(values.__len__()):
        print(values[i])


def count[*Ts: Writable](*items: *Ts):
    var total = 0
    comptime for i in range(items.__len__()):
        print(items[i])
        total += 1
    print(total)


def main():
    show(1, "two", 3.5)
    show(True)
    show()
    count("a", 2)
