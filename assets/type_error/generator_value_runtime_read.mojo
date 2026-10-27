# A generator has no run-time form: reading a generator parameter whole as
# a value is rejected, as it is not `Writable`.
# expect: an element of 'values' with type '__generator_type[Idx: Int] Int' does not conform to trait 'Writable'
def show[G: __generator_type[Idx: Int] Int]():
    print(G)


comptime Twice[x: Int] = x * 2


def main():
    show[Twice]()
