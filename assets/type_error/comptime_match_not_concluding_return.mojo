# expect: does not return a value on every path
# A `comptime match` whose cases do not cover the subject falls through
# them, so a function ending in it must still return.
def k[n: Int]() -> Int:
    comptime match n:
        case 1:
            return 1


def main():
    print(k[1]())
