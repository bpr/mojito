# A generic `def` whose clone fails to elaborate for one instantiation
# (`comptime q = [1, 2][i]` over `range(5)`) is refused with "comptime index
# 2 out of range" although only an uncalled function calls it. The pin
# instantiates only what the entry reaches and prints "ok".
def f[n: Int]():
    comptime for i in range(n):
        comptime q = [1, 2][i]
        print(q)


def unused():
    f[5]()


def main():
    print("ok")
