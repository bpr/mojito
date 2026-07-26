def choose_before[
    origin: Origin[mut=True],
    *Ts: Copyable & ImplicitlyDeletable,
](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:
    comptime for i in range(args.__len__()):
        pass
    return value


def choose_after[
    *Ts: Copyable & ImplicitlyDeletable,
    origin: Origin[mut=True],
](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:
    comptime for i in range(args.__len__()):
        pass
    return value


def add_all[
    *values: Int,
    origin: Origin[mut=True],
](ref[origin] result: Int):
    comptime for value in values:
        result += value


def main():
    var before = 40
    ref first = choose_before[origin_of(before), Int, Bool](before, 1, True)
    first += 2
    print(before)

    var after = 40
    ref second = choose_after[Int, Bool, origin=origin_of(after)](after, 1, True)
    second += 2
    print(after)

    var total = 40
    add_all[1, 1, origin=origin_of(total)](total)
    print(total)
