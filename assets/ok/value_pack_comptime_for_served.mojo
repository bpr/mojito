# A `comptime for` over a `def`'s value pack is served by the template: the
# loop header carries the pack as its sequence, and the elaborator unrolls it
# over the elements each instance binds, deciding a `comptime if` over the
# element per iteration.
def show[*vals: Int]():
    comptime for v in vals:
        print(v)


def total[*vals: Int]() -> Int:
    var sum = 0
    comptime for v in vals:
        comptime if v > 1:
            sum += v
    return sum


def main():
    show[1, 2, 3]()
    show[7]()
    print(total[1, 2, 3]())
