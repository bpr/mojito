# The pin's prelude exports `Slice` but not `ContiguousSlice` or
# `StridedSlice`, which only an import from `std.builtin.builtin_slice`
# binds. Both compilers reject the bare name: the pin with "use of unknown
# declaration 'ContiguousSlice'", Mojito as an unknown type.
def show(s: ContiguousSlice):
    print(s)

def main():
    print(1)
