# A `Codepoint` constructs from a byte, which is always a valid scalar, or
# from an unchecked `UInt32` scalar value.
def main():
    print(Codepoint(103))
    print(Codepoint(UInt8(104)))
    print(Codepoint(unsafe_unchecked_codepoint=UInt32(233)))
