# expect: function instantiation of `main` failed: constraint failed: n is four
# A closed `comptime assert` that is false fails its function's instance, as
# at the pin; the checker never judges it.

def main():
    comptime n = 3
    comptime assert n == 4, "n is four"
    print(n)
