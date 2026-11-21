# `comptime if case pattern = value` tests the pattern at compile time, as
# upstream's `comptime match` over the value: a branch binds compile-time
# values, a failed test takes the next branch or the `else`, and pattern and
# boolean branches mix in one chain. An irrefutable pattern concludes the
# chain without a coverage warning.
def classify[n: Int]():
    comptime if case 3 = n:
        print("three")
    elif case 4 = n:
        print("four")
    elif n > 10:
        print("big")
    else:
        print("other")


def boolean_first[n: Int]():
    comptime if n > 100:
        print("huge")
    elif case 3 = n:
        print("three")
    elif case (a, b) = (n, 7):
        print(a, b)


def pair[n: Int]():
    comptime if case (a, b) = (n, n + 1):
        print(a, b)
    comptime if case 2 = n:
        print("two")
    print("after")


def flag[b: Bool]():
    comptime if case True = b:
        print("true")


def first[n: Int]() -> Int:
    comptime if case x = n:
        return x
    elif case 3 = n:
        return 3
    else:
        return 0


def main():
    classify[3]()
    classify[4]()
    classify[11]()
    classify[1]()
    boolean_first[3]()
    boolean_first[200]()
    boolean_first[5]()
    pair[2]()
    pair[3]()
    flag[True]()
    flag[False]()
    print(first[4]())
