# expect: expected ']' in call argument list
# An origin-bearing reference is never a type argument: upstream reads
# `ref[origin]` as an expression followed by a stray type.
@fieldwise_init
struct RefTuple[origin: Origin[mut=True]]:
    var values: Tuple[ref[origin] Int, ref[origin] Int]


def main():
    var left = 4
    ref a = left
    ref b = left
    var pair = RefTuple((a, b))
    print(pair.values[0], pair.values[1])
