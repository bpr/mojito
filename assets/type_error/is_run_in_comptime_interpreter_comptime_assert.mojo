# expect: cannot use a dynamic value in 'comptime assert' expression
# `__is_run_in_comptime_interpreter` is a run-time value, which no
# `comptime assert` reads.


def main():
    comptime assert __is_run_in_comptime_interpreter
    print("asserted")
