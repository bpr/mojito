# expect: does not conform to trait
from std.collections import Optional


def make() -> String:
    return "seven"


def main():
    var o = Optional[Int](init_with=make)
    print(o.value())
