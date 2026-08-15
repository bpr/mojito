# expect: must be defined by a type or a Bool proposition
# A generic comptime alias body is a type expression or a Bool proposition
# (predicate alias); other value bodies stay rejected.
comptime Twice[n: Int] = 2 * n

def main():
    print(Twice[3])
