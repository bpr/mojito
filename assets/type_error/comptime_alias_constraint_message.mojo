# expect: constraint failed: single digit only
comptime Guard[n: Int]: AnyType where (n > 0, "positive only") where (n < 10, "single digit only") = Int


def main():
    var guarded: Guard[12] = 7
    print(guarded)
