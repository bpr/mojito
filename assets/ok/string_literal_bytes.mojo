# A string literal's bytes: `byte_length()` counts UTF-8 bytes and `ptr()`
# addresses the literal's static storage, for a constant receiver and for a
# literal borrowed through a parameter.
def first_byte(s: StringLiteral) -> UInt8:
    return s.ptr()[unsafe_offset=0]


def byte_count(s: StringLiteral) -> Int:
    return s.byte_length()


def main():
    print("abc".byte_length())
    print("héllo".byte_length())
    print("".byte_length())
    var p = "xyz".ptr()
    print(p[unsafe_offset=0], p[unsafe_offset=2])
    print(first_byte("A"))
    print(byte_count("four"))
