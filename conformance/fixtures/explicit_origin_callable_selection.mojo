def choose[origin: Origin[mut=True]](
    ref[origin] value: Int,
) -> ref[origin] Int:
    return value


def choose[origin: Origin[mut=True]](
    ref[origin] value: Float64,
) -> ref[origin] Float64:
    return value


def borrow[
    T: Copyable & ImplicitlyDeletable,
    origin: Origin[mut=True],
](ref[origin] value: T) -> ref[origin] T:
    return value


def main():
    var value = 39

    ref direct = choose[origin_of(value)](value)
    direct += 1

    var selected: def(ref[origin_of(value)] Int) thin -> ref[origin_of(value)] Int = choose[origin_of(value)]
    ref contextual = selected(value)
    contextual += 1

    var generic = borrow[Int, origin_of(value)]
    ref result = generic(value)
    result += 1
    print(value)
