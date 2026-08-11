# expect: a generic comptime alias must be defined by a type
comptime Twice[n: Int] = 2 * n


def main():
    print(1)
