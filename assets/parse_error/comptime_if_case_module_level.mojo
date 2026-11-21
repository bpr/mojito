# expect: 'comptime if' must be contained in a function
# A module-scope `comptime if case` is rejected as a `comptime if` is.
comptime N = 3
comptime if case 3 = N:
    comptime M = 1


def main():
    pass
