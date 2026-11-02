# `Bool.__int__` is the declared member of upstream's `struct Bool`, so its
# type-receiver call takes a `Bool` receiver and rejects an `Int8` one.
# expect: value passed to 'self' of 'Bool.__int__'


def main():
    print(Bool.__int__(Int8(1)))
