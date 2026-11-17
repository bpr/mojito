# A run-time local holding a function has a thin function type, which does
# not conform to an applied generic struct's initializer `def(...)` bound:
# no initializer is viable, as at the pin.
# expect: value passed to 'init_with' cannot be converted from 'def() thin -> Int' to 'F'


def make() -> Int:
    return 41


def main():
    var f = make
    print(Optional[Int](init_with=f).value())
