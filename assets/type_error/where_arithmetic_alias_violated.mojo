# expect: violated constraint
# A generic alias's arithmetic `where` clause is decided at its application.
comptime Next[n: Int, m: Int]: Int where n + 1 == m = m


def main():
    print(Next[1, 3])
