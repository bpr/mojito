# expect: cannot use a dynamic value in 'comptime if' condition
# `__is_run_in_comptime_interpreter` is a run-time value, which no
# `comptime if` condition reads.


def main():
    comptime if __is_run_in_comptime_interpreter:
        print("interpreted")
