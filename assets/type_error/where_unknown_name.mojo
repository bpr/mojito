# A `where` clause naming no declaration is rejected at the declaration,
# not left to fail at a call.
# expect: not a compile-time Int constant: m
def f[n: Int]() where m > 0:
    print("ok")


def main():
    pass
