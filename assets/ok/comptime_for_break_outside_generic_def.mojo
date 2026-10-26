# A compile-time `break` or `continue` leaves or advances a `comptime for`
# wherever the loop stands: in a non-generic `def`, a method, over a
# compile-time list, over a pack's indices, beside a local `comptime`
# binding, and a run-time `break` in an unrolled body. Output matches the
# pin.
def plain():
    comptime for i in range(5):
        comptime if i == 1:
            continue
        comptime if i == 3:
            break
        print(i)


@fieldwise_init
struct S:
    def m(self):
        comptime for i in range(5):
            comptime if i == 3:
                break
            print("m", i)


def over_list():
    comptime xs = [10, 20, 30, 40]
    comptime for x in xs:
        comptime if x == 30:
            break
        print("l", x)


def over_pack[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        comptime if i == 1:
            continue
        print("p", args[i])


def with_local():
    comptime for i in range(4):
        comptime k = i * 2
        comptime if k == 4:
            break
        print("k", k)


def rt_break():
    comptime for i in range(5):
        if i == 2:
            break
        print("r", i)


def main():
    plain()
    S().m()
    over_list()
    over_pack(1, "a", 2.5)
    with_local()
    rt_break()
