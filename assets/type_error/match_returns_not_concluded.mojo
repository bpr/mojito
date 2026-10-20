# expect: does not return a value on every path
# A match whose last case is refutable may match nothing, so returns in every
# case do not conclude the function.
def f(n: Int) -> Int:
    match n:
        case 1:
            return 1
        case 2:
            return 2


def main():
    print(f(1))
