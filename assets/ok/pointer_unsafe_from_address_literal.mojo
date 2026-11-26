# A pointer upstream's `Pointer(unsafe_from_address=)` builds from a literal
# address holds that address: its `Int` is the literal, natively as on the VM.


def main():
    print(Int(Pointer[Int, MutAnyOrigin](unsafe_from_address=4096)))
