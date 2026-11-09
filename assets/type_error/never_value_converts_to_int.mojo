# A `Never` value does not convert implicitly to a value type: a call whose
# result is `Never` ends its path only as a statement. The pin reports
# "cannot implicitly convert 'Never' value to 'Int'".
# expect: expected Int, found Never
from std.os import abort


def f(x: Int) -> Int:
    if x > 0:
        return x
    return abort("no")


def main():
    print(f(1))
