# expect: cannot use a dynamic value in comptime initializer
# `__is_run_in_comptime_interpreter` is a run-time value, which no
# `comptime` initializer reads, alone or under an operator.


def both[b: Bool]() -> Bool:
    comptime i = __is_run_in_comptime_interpreter and b
    return i


def main():
    print(both[True]())
