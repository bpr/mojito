# expect: violated constraint
comptime twice[n: Int] where n > 0 = n * 2


def main():
    print(twice[0])
