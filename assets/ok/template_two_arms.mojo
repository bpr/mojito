# A `def` keyed on a `Bool` value parameter: source validation checks both
# `comptime if` arms once, and each instance inherits the facts of the arm the
# elaborator selected (class ScalarBranches). The untaken arm contributes
# nothing executable.
def choose[flag: Bool]() -> Int:
    comptime if flag:
        return 11
    else:
        return 22


def main():
    print(choose[True]())
    print(choose[False]())
