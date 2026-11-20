# A `Pointer` in another address space is another type: it does not
# convert implicitly to a generic `Pointer`, as at the pin.
# expect: address_space=AddressSpace.LOCAL
from std.memory import AddressSpace


def main():
    var x = 5
    var p = Pointer(to=x)
    var y: Pointer[Int, origin_of(x)] = p.unsafe_address_space_cast[
        AddressSpace.LOCAL
    ]()
    print(y[])
