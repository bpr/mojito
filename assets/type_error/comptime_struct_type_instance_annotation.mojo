# A struct type bound where its annotation declares an instance of that same
# struct is rejected, and the pin suggests constructing it.
# expect: cannot implicitly convert 'Int' type as a value to an instance of 'Int'
comptime x: Int = Int


def main():
    pass
