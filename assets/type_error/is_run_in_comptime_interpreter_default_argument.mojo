# expect: cannot use a dynamic value in default argument
# `__is_run_in_comptime_interpreter` is a run-time value, which no parameter
# default reads: a default is evaluated at compile time.


def show(flag: Bool = __is_run_in_comptime_interpreter) -> Bool:
    return flag


def main():
    print(show())
