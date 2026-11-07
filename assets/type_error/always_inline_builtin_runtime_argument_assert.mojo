# expect: constraint failed: positive
# A call of an `@always_inline("builtin")` callable with a run-time argument
# is no compile-time fold: the instance it calls judges the body's
# `comptime assert`, as the pinned Mojo's does.


@always_inline("builtin")
def checked[k: Int](n: Int) -> Int:
    comptime assert k > 0, "positive"
    return n + 1


def main():
    var x = 1
    print(checked[-1](x))
