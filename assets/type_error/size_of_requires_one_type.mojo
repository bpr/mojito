# expect: type 'size_of' expects 1 type argument(s), got 0
from std.sys import size_of


def main():
    print(size_of())
